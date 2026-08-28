//! A hard edge drawn round a highlighted sprite's silhouette, and the glow
//! behind it.
//!
//! The second way of saying "the cursor is on this". The first is
//! [`items::HIGHLIGHT_HUE`](crate::items::HIGHLIGHT_HUE), which replaces the
//! art's colour; this draws a ring round its shape instead, and the two compose
//! because they happen in different passes over different pixels.
//!
//! # One texture, and what is made of it
//!
//! 1. [`SpriteRenderer::render_mask`](crate::renderer::SpriteRenderer::render_mask)
//!    draws the sprites to be outlined into an `R8Uint` mask the size of the
//!    world image, each in its own id — `silhouette.wgsl`. It tests the world's
//!    depth buffer, so the mask holds what is *visible*.
//! 2. When there is a [`Glow`], the same mask is seeded as coverage at half that
//!    size and spread by three Kawase iterations — `glow.wgsl`. The ids are
//!    dropped here on purpose: two objects need two *rings*, and two glows that
//!    meet should pool.
//! 3. [`Outline::render`] runs after the blit, over the same rectangle of the
//!    surface, and turns "this texel borders a different id" into a coloured
//!    ring — `outline.wgsl` — adding the spread underneath it. One pass for both
//!    halves, which is what the premultiplied blend buys.
//!
//! Nothing is precomputed and nothing is stored per graphic. The cost is one
//! small draw plus one pass over the viewport, and it is the same whether the
//! atlas holds ten sprites or ten thousand — which is the argument for doing it
//! here rather than baking an edge into the art, where the work would scale with
//! the art instead of with what is lit.
//!
//! # Why the thickness is in virtual pixels
//!
//! The mask is the world image's size, so its texels are the world's *virtual*
//! pixels and the ring is magnified by the blit along with the art it surrounds.
//! That is on purpose. A one-*screen*-pixel hairline round a sprite drawn at 4×
//! is a line finer than any edge in the picture it is tracing, and it reads as a
//! rendering artefact rather than as a highlight; a ring one art pixel thick
//! grows with the sprite and stays part of the same picture.
//!
//! It also costs nothing extra: the ring is found where the mask already is, and
//! the blit's nearest sampler carries it up whole.
//!
//! `docs/outline.md` is the plan this is built against.

/// The format of the mask the silhouette pass writes and this one reads.
///
/// `R8Uint` and not `R8Unorm`: what is stored is an *identity*, not a coverage,
/// and comparing two ids for equality through a normalised float would be
/// comparing `n / 255.0` values — right for 255 objects and wrong-looking in
/// every code review that follows. Uint also means no sampler: the ring reads
/// exact texels with `textureLoad`, which is the only correct thing to do to a
/// mask of numbers.
pub const MASK_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R8Uint;

/// How many sprites can be outlined at once, each with its own ring.
///
/// The mask holds one byte and zero means "nothing here", so this is what is
/// left. The client lights one item at a time today; the ceiling exists so the
/// silhouette pass can say what happens past it rather than wrapping ids round
/// and ringing two objects as one.
pub const MAX_OUTLINED: usize = 255;

/// Bytes of `outline.wgsl`'s uniform block: three `vec4`s.
const RING_BYTES: u64 = 48;

/// Bytes of `glow.wgsl`'s: one `vec4`, of which one component is read.
const STEP_BYTES: u64 = 16;

/// How many Kawase iterations the glow is spread over.
///
/// Three, because the reach doubles with each one and three at half resolution
/// already covers a quarter of a sprite. It is a constant rather than a knob:
/// the iteration count decides how *smooth* the falloff is and the offsets
/// decide how *far* it goes, and only the second of those is worth a caller's
/// attention — see [`Glow::radius`].
const GLOW_PASSES: usize = 3;

/// What the spread silhouette is kept in.
///
/// Four channels for one, and deliberately: a single-channel target is what the
/// data wants, and `R8Unorm` as a colour attachment is the one format in this
/// pipeline whose fragment output type has to be written differently from every
/// other pass here. The whole chain is a quarter of the mask's area, so the
/// three wasted channels cost less than the mask itself does.
const GLOW_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Create the mask the silhouette pass draws into, at the world image's size.
///
/// Here rather than in the caller for the reason
/// [`blit::world_texture`](crate::blit::world_texture) is: the format and the
/// usage are this module's decision, and a texture created without
/// `TEXTURE_BINDING` fails at bind-group time with an error naming neither pass.
///
/// **The size is the world image's, not the surface's.** The silhouette pass
/// shares the world's depth buffer, and a depth attachment must match its colour
/// attachment exactly.
pub fn mask_texture(device: &wgpu::Device, width: u32, height: u32) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("outline mask"),
        size: wgpu::Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: MASK_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    })
}

/// The soft half of a highlight: the silhouette, spread and added to the
/// picture rather than drawn over it.
///
/// The ring says *exactly* where the thing is and the glow says *there is
/// something here*, which is the half that survives being looked at out of the
/// corner of an eye. They are one mask and one composite — `docs/outline.md` D5
/// — so a glow costs the blur chain and nothing else.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Glow {
    /// How far the light reaches past the silhouette, in mask texels — that is,
    /// in virtual pixels, the same unit [`Ring::width`] is in and for the same
    /// reason.
    ///
    /// It is a *reach* and not a radius of a kernel: the falloff is smooth all
    /// the way out, so there is no distance at which the glow stops rather than
    /// fades. Six is about a seventh of a 44-pixel static.
    pub radius: u32,
    /// Its colour, and `a` is how bright the light is where it leaves the
    /// silhouette. Added to what is already there, so an alpha above one is a
    /// glow that blows the picture out rather than a brighter one.
    pub color: [f32; 4],
}

impl Glow {
    /// A soft white halo, matching [`Ring::DEFAULT`]'s edge.
    pub const DEFAULT: Self = Self {
        radius: 6,
        // Well under one: the glow is added on top of a finished picture, and
        // half of white over the middle greys of UO art is already the
        // difference between "lit" and "not" at a glance.
        color: [1.0, 1.0, 1.0, 0.45],
    };
}

/// What one ring looks like.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Ring {
    /// Its colour, straight through to the surface — no hue ramp and no
    /// lighting. A highlight that dimmed at night would be a highlight that
    /// stops working exactly when the picture is hardest to read, which is why
    /// this pass runs *after* the blit rather than into the world image.
    pub color: [f32; 4],
    /// Its half-width, in the mask's texels — that is, in virtual pixels. One
    /// is a one-pixel edge round the silhouette, drawn at the art's own scale.
    ///
    /// Above one the neighbourhood is searched densely, so the cost is
    /// `(2n+1)²` taps a fragment. Two is already a heavy ring on 44-pixel art.
    ///
    /// [`Ring::for_zoom`] is what sets it in the client: one texel is one
    /// *screen* pixel only while the world is magnified.
    pub width: u32,
    /// The halo round the edge, or `None` for the hard edge alone.
    pub glow: Option<Glow>,
}

impl Ring {
    /// A white one-pixel edge, and nothing else.
    ///
    /// White rather than a colour of its own: the ring's job is to be *not the
    /// picture*, and UO art has no white in its silhouettes worth speaking of.
    /// A caller wanting the reference's yellow-green highlight has the field.
    pub const DEFAULT: Self = Self {
        color: [1.0, 1.0, 1.0, 1.0],
        width: 1,
        glow: None,
    };

    /// The same edge with [`Glow::DEFAULT`] behind it — what the client draws.
    ///
    /// The hard edge is kept under the halo rather than replaced by it: a glow
    /// alone says roughly where something is, and picking a barrel out of a
    /// stack of three needs the edge to say which one.
    pub const SOFT: Self = Self {
        glow: Some(Glow::DEFAULT),
        ..Self::DEFAULT
    };

    /// What a click is *holding* — a persistent ring, so it does not answer
    /// the same question the hover ring does and cannot be confused with it.
    /// Orange rather than white: the hover ring already owns white, and the
    /// two are drawn in the same frame whenever the cursor is over something
    /// other than what was clicked.
    pub const SELECTED: Self = Self {
        color: [1.0, 0.6, 0.0, 1.0],
        width: 1,
        glow: Some(Glow {
            radius: 6,
            color: [1.0, 0.6, 0.0, 0.45],
        }),
    };

    /// The same ring, thickened enough to survive being minified.
    ///
    /// The mask is the world image and the composite reads it at the surface's
    /// resolution, so below 1:1 the ring is *point-sampled*: at `1/2` every
    /// other mask texel is never looked at, and a one-texel ring comes out as a
    /// dashed line rather than a thin one. Widening it to the zoom's
    /// denominator over its numerator — two texels at every rung below 1:1 —
    /// puts at least one ring texel in every screen pixel's footprint.
    ///
    /// Magnifying it is the identity: one texel is already several screen
    /// pixels, which is D4's whole argument.
    pub fn for_zoom(self, zoom: crate::camera::Zoom) -> Self {
        Self {
            width: self.width * zoom.denominator().div_ceil(zoom.numerator()).max(1),
            ..self
        }
    }
}

/// Where one ring pass draws, and from what.
#[derive(Clone, Copy, Debug)]
pub struct Frame<'a> {
    /// What to draw onto — the surface, after the blit has put the world there.
    pub target: &'a wgpu::TextureView,
    /// The mask the silhouette pass filled.
    pub mask: &'a wgpu::TextureView,
    /// Its size in texels. Carried beside the view for the reason
    /// [`Target`](crate::renderer::Target) carries a size: a view does not know
    /// its own extent, and a mask sampled at the wrong one is a ring drawn in
    /// the wrong place — which looks like a projection bug and is not one.
    pub mask_size: (u32, u32),
    /// The rectangle of `target` the world was blitted into. The same
    /// [`ViewportRect`](crate::blit::ViewportRect) the blit was given, or the
    /// ring lands somewhere the world is not.
    pub rect: crate::blit::ViewportRect,
}

/// Turns a silhouette mask into a ring on the surface.
#[derive(Debug)]
pub struct Outline {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    uniforms: wgpu::Buffer,
    /// The glow's two pipelines: coverage out of the id mask, then one Kawase
    /// iteration, run [`GLOW_PASSES`] times between the cached pair.
    seed: wgpu::RenderPipeline,
    seed_layout: wgpu::BindGroupLayout,
    blur: wgpu::RenderPipeline,
    blur_layout: wgpu::BindGroupLayout,
    /// Linear and clamped: the taps are deliberately between texels — that is
    /// where a Kawase iteration gets four texels out of one tap — and a tap off
    /// the edge must read the edge rather than wrap the glow to the far side of
    /// the screen.
    sampler: wgpu::Sampler,
    /// One offset per iteration. Separate buffers and not one written between
    /// passes: every pass of a frame is recorded into one encoder and submitted
    /// together, so a buffer rewritten between them would have its last value in
    /// all of them.
    steps: Vec<wgpu::Buffer>,
    /// The blur's ping-pong pair, at half the mask's size, and what size that
    /// was. Owned here rather than by the caller: they are this module's
    /// intermediates, nothing else can draw into them, and a caller that had to
    /// resize them would be a caller that can get it wrong.
    spread: SpreadState,
}

/// Whether the glow targets have been fitted to a frame yet.
///
/// An [`Outline`] has no mask size when it is constructed. That is a real
/// lifecycle state rather than an absent value: the first frame fits the pair,
/// and later frames either reuse it or replace it after a resize.
#[derive(Debug)]
enum SpreadState {
    Unfitted,
    Fitted(Spread),
}

/// The pair the glow is ping-ponged between, and the mask size they were made
/// for.
#[derive(Debug)]
struct Spread {
    /// The mask size these were sized from — not their own size, so the test
    /// for "still valid" is against what the caller passes.
    mask: (u32, u32),
    textures: [wgpu::Texture; 2],
}

impl SpreadState {
    /// Fit the pair to `mask` and return views for this frame's passes.
    fn fit(&mut self, device: &wgpu::Device, mask: (u32, u32)) -> [wgpu::TextureView; 2] {
        if let Self::Fitted(spread) = self {
            if spread.mask == mask {
                return spread.views();
            }
        }

        let spread = Spread::new(device, mask);
        let views = spread.views();
        *self = Self::Fitted(spread);
        views
    }
}

impl Spread {
    /// Build the pair at half the mask's size in each direction, rounded up.
    ///
    /// Half resolution is enough because the glow is a falloff several texels
    /// wide and the composite reads it through a linear sampler. It is also
    /// what makes three iterations enough: every texel here reaches two in the
    /// world's mask.
    fn new(device: &wgpu::Device, mask: (u32, u32)) -> Self {
        let size = wgpu::Extent3d {
            width: mask.0.div_ceil(2).max(1),
            height: mask.1.div_ceil(2).max(1),
            depth_or_array_layers: 1,
        };
        let target = |label| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: GLOW_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
        };
        Self {
            mask,
            textures: [target("glow a"), target("glow b")],
        }
    }

    fn views(&self) -> [wgpu::TextureView; 2] {
        self.textures
            .each_ref()
            .map(|texture| texture.create_view(&wgpu::TextureViewDescriptor::default()))
    }
}

impl Outline {
    /// Build the pipeline for a target of `format` — the surface's, not
    /// [`WORLD_FORMAT`](crate::blit::WORLD_FORMAT): this pass draws over the
    /// blit's output.
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("outline"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Uint,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ring"),
            size: RING_BYTES,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("outline"),
            source: wgpu::ShaderSource::Wgsl(include_str!("outline.wgsl").into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("outline"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("outline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    // Blended, unlike every other pass here, because this one
                    // draws *onto* a finished picture rather than into an empty
                    // target: a ring at less than full alpha is a ring the world
                    // shows through, which is what a soft highlight is made of.
                    //
                    // Premultiplied and not straight alpha, which is what lets
                    // one pass draw both halves: `dst = src.rgb + dst *
                    // (1 - src.a)`, so a fragment with a colour and *no* alpha
                    // is pure addition — the glow — and one with both is the
                    // ordinary blend — the ring. Straight alpha would need a
                    // second pass with a second blend state for the glow, and
                    // then a second target to read it from.
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
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
            // No depth: the world's depth buffer already decided what is
            // visible, and the mask is the record of that decision.
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let glow = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("glow"),
            source: wgpu::ShaderSource::Wgsl(include_str!("glow.wgsl").into()),
        });

        // Binding 0 alone: the seed reads the id mask and nothing else, and its
        // output size is the target's rather than a uniform's.
        let seed_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("glow seed"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Uint,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
        });
        let seed = full_screen_pipeline(device, &glow, "seed", &seed_layout, GLOW_FORMAT);

        // 1..3, matching `glow.wgsl` — see the comment there for why the blur's
        // bindings start where they do.
        let blur_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("glow blur"),
            entries: &[
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
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let blur = full_screen_pipeline(device, &glow, "blur", &blur_layout, GLOW_FORMAT);

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("glow"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let steps = (0..GLOW_PASSES)
            .map(|_| {
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("glow step"),
                    size: STEP_BYTES,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                })
            })
            .collect();

        Self {
            pipeline,
            layout,
            uniforms,
            seed,
            seed_layout,
            blur,
            blur_layout,
            sampler,
            steps,
            spread: SpreadState::Unfitted,
        }
    }

    /// Draw the ring over `frame.rect`, keeping everything else.
    ///
    /// Loads rather than clears: the blit has just written the world there, and
    /// this adds an edge to it.
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        frame: Frame<'_>,
        ring: Ring,
    ) {
        let spread = self.spread.fit(device, frame.mask_size);
        let mut bytes = Vec::with_capacity(RING_BYTES as usize);
        for value in [
            frame.mask_size.0 as f32,
            frame.mask_size.1 as f32,
            ring.width.max(1) as f32,
            ring.glow.map_or(0.0, |glow| glow.color[3]),
        ] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        for channel in ring.color {
            bytes.extend_from_slice(&channel.to_le_bytes());
        }
        for channel in ring.glow.map_or([0.0; 4], |glow| glow.color) {
            bytes.extend_from_slice(&channel.to_le_bytes());
        }
        queue.write_buffer(&self.uniforms, 0, &bytes);

        // The blur chain, into the pair above. Skipped when there is no glow —
        // the strength written just now is then zero, so the composite never
        // reads what is in there.
        if let Some(glow) = ring.glow {
            self.spread_mask(device, queue, encoder, frame.mask, &spread, glow);
        }
        let lit = &spread[GLOW_PASSES % 2];

        // A bind group per call, as the blit does: the mask is recreated on
        // every resize and every zoom step, and a cached group would point at a
        // texture nothing is drawing into any more.
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("outline"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(frame.mask),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: self.uniforms.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(lit),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("outline"),
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

        if frame.rect.width == 0 || frame.rect.height == 0 {
            return;
        }

        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.set_viewport(
            frame.rect.x as f32,
            frame.rect.y as f32,
            frame.rect.width as f32,
            frame.rect.height as f32,
            0.0,
            1.0,
        );
        pass.draw(0..4, 0..1);
    }

    /// Coverage out of the id mask, then [`GLOW_PASSES`] Kawase iterations
    /// between the pair, leaving the result in `textures[GLOW_PASSES % 2]`.
    ///
    /// Every pass clears rather than loads: each one covers its whole target,
    /// and a load would be asking the driver to fetch pixels that are about to
    /// be overwritten — the pair holds the *previous* frame's glow, and on a
    /// tiled GPU that fetch is the expensive half of a small pass.
    fn spread_mask(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        mask: &wgpu::TextureView,
        views: &[wgpu::TextureView; 2],
        glow: Glow,
    ) {
        let seed = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("glow seed"),
            layout: &self.seed_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(mask),
            }],
        });
        full_screen_pass(encoder, "glow seed", &views[0], &self.seed, &seed);

        for (index, offset) in step_offsets(glow.radius).into_iter().enumerate() {
            let step = [offset, 0.0, 0.0, 0.0];
            let mut bytes = Vec::with_capacity(STEP_BYTES as usize);
            for value in step {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
            queue.write_buffer(&self.steps[index], 0, &bytes);
            let source = index % 2;
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("glow blur"),
                layout: &self.blur_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&views[source]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: self.steps[index].as_entire_binding(),
                    },
                ],
            });
            full_screen_pass(encoder, "glow blur", &views[1 - source], &self.blur, &bind_group);
        }
    }
}

/// The Kawase offsets for a reach of `radius` mask texels, in the blur's own
/// half-resolution texels.
///
/// Growing 1:2:3 rather than constant, which is what makes the falloff smooth
/// rather than a stack of identical box filters, and scaled so that the sum of
/// them — the distance the outermost tap reaches — comes back out at `radius`
/// once the half resolution is undone.
///
/// Never below half a texel: a tap closer than that to the centre lands in the
/// texel it started in, and four of those are one texel read four times. That is
/// an iteration that does nothing, and a radius small enough to ask for it wants
/// no glow rather than a broken one.
fn step_offsets(radius: u32) -> [f32; GLOW_PASSES] {
    // 1 + 2 + 3 in half-resolution texels is 12 of the mask's.
    let unit = radius as f32 / 12.0;
    std::array::from_fn(|index| (unit * (index + 1) as f32).max(0.5))
}

/// One full-screen draw of `pipeline` into `target`, clearing it first.
fn full_screen_pass(
    encoder: &mut wgpu::CommandEncoder,
    label: &str,
    target: &wgpu::TextureView,
    pipeline: &wgpu::RenderPipeline,
    bind_group: &wgpu::BindGroup,
) {
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: target,
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
    });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, bind_group, &[]);
    pass.draw(0..4, 0..1);
}

/// A pipeline that draws `entry`'s fragment over a whole target, with no vertex
/// buffer and no depth — the shape every pass in the glow chain has.
fn full_screen_pipeline(
    device: &wgpu::Device,
    shader: &wgpu::ShaderModule,
    entry: &str,
    layout: &wgpu::BindGroupLayout,
    format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(entry),
        bind_group_layouts: &[Some(layout)],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(entry),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(entry),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                // No blending anywhere in the chain: every pass writes every
                // texel of its target, so there is nothing underneath to blend
                // with. The one blended pass is the composite.
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
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    })
}
