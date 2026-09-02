//! The pass that draws [`Solid`](crate::solid::Solid)s over a finished frame.
//!
//! `docs/archive/render/lighting.md` step 23.0. The first version of that step painted through
//! the client's egui layer, which put the instrument somewhere no test could
//! reach: `render`'s pictures are rendered headless into a texture and read back
//! — `tests/cost.rs` draws Britain at the widest zoom and writes the frame out —
//! and an overlay drawn by the UI toolkit appears in none of them. A view whose
//! whole job is to be looked at should be capturable by the thing that takes the
//! pictures.
//!
//! # What is on the GPU and what is not
//!
//! **The projection is not.** The corners arrive in viewport pixels, from
//! [`Solid::faces`](crate::solid::Solid::faces), which is Rust with a test that
//! a unit solid's top face *is* its tile's diamond. A vertex shader that
//! projected a box from its two corners would be a second implementation of the
//! arithmetic every sprite in the frame is placed by, and decision 39.7's
//! half-tile trap is exactly the kind of thing that then differs in one of them.
//! What the GPU does here is what only it can: blend a hundred thousand
//! translucent fragments over the picture.
//!
//! The cost of that choice is one buffer write per frame — 24 bytes a vertex,
//! 18 vertices and 18 outline points per solid — and it is measured rather than
//! argued: see `tests/cost.rs`'s `solids` case.
//!
//! A stationary diagnostic view does not make the corners move.  Its assembled
//! bytes therefore stay beside the renderer and are reused until the camera,
//! viewport, visible solid list, or edge style changes.  The small uniform is
//! still written each frame because the surface size can change independently.
//!
//! # Where it runs
//!
//! **After the blit**, on the surface, like [`Ring`](crate::outline::Ring) and
//! for its reason: a diagnostic that dimmed at night would stop working exactly
//! when the picture is hardest to read. It writes no depth and reads none — the
//! translucent half of decision 39.2 — so the box is visible *through* whatever
//! stands in front of it, which for an instrument is the wanted answer rather
//! than a compromise.

use std::sync::Arc;

use crate::blit::ViewportRect;
use crate::camera::{
    Camera,
    RealPoint,
};
use crate::solid::Solid;

/// How much of the picture a face keeps under a solid.
///
/// The art has to stay readable under the box, or the view has hidden the thing
/// it is a claim about. Lower than the wireframe's fill, because a solid covers
/// three faces where a plane covered one.
const FILL_ALPHA: f32 = 0.42;

/// The near-black the silhouette is stroked in, premultiplied by nothing — the
/// shader does that.
const EDGE: [f32; 4] = [0.04, 0.03, 0.06, 0.85];

/// Bytes one vertex takes: a position and a colour.
const VERTEX_BYTES: usize = 6 * 4;

/// The uniform block, which is one `vec4` of screen size and padding.
const UNIFORM_BYTES: u64 = 16;

/// How many vertices a buffer starts out able to hold.
///
/// A thousand solids' worth of fills, which is the order a town at 1:1 produces.
/// It grows by doubling from here, and the town at the widest zoom is a few
/// doublings up.
const INITIAL_VERTICES: u64 = 18_000;

/// What one run of this pass draws over, and where.
///
/// The same shape [`blit::Frame`](crate::blit::Frame) has and for its reason:
/// the target, its size and the rectangle the world occupies inside it always
/// travel together, and a `render` that took them one by one would be a call
/// whose arguments are told apart by position alone.
#[derive(Clone, Copy, Debug)]
pub struct Frame<'a> {
    /// The picture to draw over — the surface, after the blit has lit it.
    pub target: &'a wgpu::TextureView,
    /// The whole target's size in physical pixels, which is what turns a pixel
    /// into clip space.
    pub size:   (u32, u32),
    /// Where the world's own picture sits inside it. The corners a solid hands
    /// back are measured from *its* corner, and this is what puts them on the
    /// surface — it is also the scissor, so a box near the edge of the world
    /// cannot paint over the HUD strip beside it.
    pub rect:   ViewportRect,
}

/// One RGB colour per corner of a face, in [`Solid::faces`]'s own four-corner
/// ring order.
///
/// [`SolidsRenderer::render_lit`]'s colour-per-face was one value smeared flat
/// over the whole triangle pair; this is the same claim made at each corner
/// instead, so the ordinary vertex interpolation `solids.wgsl` already does —
/// nothing new in the shader — turns it into a gradient across the face rather
/// than a step at its edge. Still an approximation of the real, non-linear
/// falloff [`crate::light::sample`] computes (the docs on [`crate::light::Reach`]
/// have the `(1 - d)²` term) — four points linearly blended, not the function
/// itself — but the step this replaces was the same approximation at *one*
/// point.
pub type FaceColours = [[f32; 3]; 4];

/// The two things a diagnostic caller wants to change about how the fills and
/// the outline draw — bundled into one argument rather than two, since a
/// third would otherwise have to become a fourth positional `bool` nothing at
/// the call site names. `edges` the live overlay never changes; `opaque` it
/// does, through its own checkbox.
#[derive(Clone, Copy, Debug)]
pub struct Style {
    /// Stroke the silhouette and the interior star over the fills.
    ///
    /// Off, what is left is the fills alone, unmixed with a line's own colour —
    /// see [`SolidsRenderer::render`]'s own doc for why that is what shows a
    /// face wrongly surviving behind one that should have hidden it.
    pub edges:  bool,
    /// A straight overwrite instead of blended in.
    ///
    /// A later, nearer face then genuinely hides an earlier, farther one
    /// instead of tinting through it at [`FILL_ALPHA`] — see
    /// [`SolidsRenderer::render`]'s own doc for what this does and does not
    /// fix.
    pub opaque: bool,
}

impl Default for Style {
    /// The live F5 overlay's own two answers: stroked, and translucent.
    fn default() -> Self {
        Self {
            edges:  true,
            opaque: false,
        }
    }
}

/// The pass, and the buffers it reuses between frames.
#[derive(Debug)]
pub struct SolidsRenderer {
    fills:        wgpu::RenderPipeline,
    /// [`Self::fills`], but a straight overwrite instead of blended in.
    ///
    /// For a caller that wants what it draws *last* to actually hide what it
    /// draws over — [`Self::render`]'s and [`Self::render_lit`]'s `opaque` — and
    /// nothing else about the pipeline differs, so this exists rather than a
    /// runtime blend state, which `wgpu` bakes into the pipeline and cannot
    /// switch per draw.
    fills_opaque: wgpu::RenderPipeline,
    edges:        wgpu::RenderPipeline,
    bind_group:   wgpu::BindGroup,
    uniforms:     wgpu::Buffer,
    vertices:     wgpu::Buffer,
    capacity:     u64,
    /// CPU vertices last uploaded to [`Self::vertices`].
    ///
    /// The solids overlay often stays open over a stationary world while it is
    /// inspected. Reprojecting thousands of corners and sending the same bytes
    /// to the GPU every frame is then pure work, so the last complete answer is
    /// retained until one of its inputs changes.
    cached:       Option<CachedVertices>,
}

#[derive(Debug)]
struct CachedVertices {
    camera:     Camera,
    rect:       ViewportRect,
    solids:     Vec<(Solid, [f32; 3])>,
    edges:      bool,
    bytes:      Arc<[u8]>,
    fill_bytes: usize,
    drawn:      usize,
}

impl SolidsRenderer {
    /// Build the three pipelines: filled faces, the same opaque, and the
    /// silhouette over them.
    ///
    /// One shader throughout — a triangle list blended, the same triangle list
    /// not, and a line list, and nothing but the topology and the blend state
    /// tells the three apart.
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label:   Some("solids"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding:    0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty:         wgpu::BindingType::Buffer {
                    ty:                 wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size:   None,
                },
                count:      None,
            }],
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label:              Some("solids"),
            size:               UNIFORM_BYTES,
            usage:              wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label:   Some("solids"),
            layout:  &layout,
            entries: &[wgpu::BindGroupEntry {
                binding:  0,
                resource: uniforms.as_entire_binding(),
            }],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label:  Some("solids"),
            source: wgpu::ShaderSource::Wgsl(include_str!("solids.wgsl").into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label:              Some("solids"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size:     0,
        });
        let pipeline = |topology, label, blend| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label:          Some(label),
                layout:         Some(&pipeline_layout),
                vertex:         wgpu::VertexState {
                    module:              &shader,
                    entry_point:         Some("vs_main"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    buffers:             &[Some(wgpu::VertexBufferLayout {
                        array_stride: VERTEX_BYTES as u64,
                        step_mode:    wgpu::VertexStepMode::Vertex,
                        attributes:   &[
                            wgpu::VertexAttribute {
                                format:          wgpu::VertexFormat::Float32x2,
                                offset:          0,
                                shader_location: 0,
                            },
                            wgpu::VertexAttribute {
                                format:          wgpu::VertexFormat::Float32x4,
                                offset:          8,
                                shader_location: 1,
                            },
                        ],
                    })],
                },
                fragment:       Some(wgpu::FragmentState {
                    module:              &shader,
                    entry_point:         Some("fs_main"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    targets:             &[Some(wgpu::ColorTargetState {
                        format,
                        blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive:      wgpu::PrimitiveState {
                    topology,
                    strip_index_format: None,
                    front_face: wgpu::FrontFace::Ccw,
                    // A face is emitted in whichever winding its corners came in
                    // and is never seen from behind: there is no rotation in this
                    // renderer, so the three faces drawn are the three visible.
                    cull_mode: None,
                    unclipped_depth: false,
                    polygon_mode: wgpu::PolygonMode::Fill,
                    conservative: false,
                },
                // No depth, either read or written — see the module doc. Opaque
                // draws still occlude correctly without one: painted back to
                // front (`solid::standing`'s own order), a later, nearer face
                // simply overwrites every pixel of an earlier, farther one.
                depth_stencil:  None,
                multisample:    wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache:          None,
            })
        };
        Self {
            // Premultiplied, like the ring pass: this draws onto a finished
            // picture and the picture has to show through.
            fills: pipeline(
                wgpu::PrimitiveTopology::TriangleList,
                "solids fills",
                Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
            ),
            fills_opaque: pipeline(wgpu::PrimitiveTopology::TriangleList, "solids fills opaque", None),
            edges: pipeline(
                wgpu::PrimitiveTopology::LineList,
                "solids edges",
                Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
            ),
            bind_group,
            uniforms,
            vertices: new_vertex_buffer(device, INITIAL_VERTICES),
            capacity: INITIAL_VERTICES,
            cached: None,
        }
    }

    /// Draw `solids` over `view`, which is a target of `width` by `height` real
    /// pixels that something has already drawn a picture into.
    ///
    /// Each entry is a box and the colour of its kind — see
    /// [`solid::standing`](crate::solid::standing), which is what builds the
    /// list from a grid, so that two callers cannot disagree about what is on
    /// screen. Back to front is the caller's; this draws them in order.
    ///
    /// Answers **how many were drawn** — those left after the ones outside the
    /// viewport are dropped. A view that silently draws a fraction of a grid
    /// looks exactly like a grid with little in it, which is the one failure an
    /// instrument may not have, so the number is handed back to be shown rather
    /// than kept.
    ///
    /// `style.edges` strokes the silhouette and the interior star over the
    /// fills — off, what is left is the fills alone, unmixed with a line's own
    /// colour.
    ///
    /// `style.opaque` swaps the blended fill pipeline for a straight-overwrite
    /// one — a later, nearer face then genuinely hides an earlier, farther one
    /// instead of tinting through it at [`FILL_ALPHA`], the translucency this
    /// is normally drawn with so a box stays visible through whatever the
    /// picture already stood in front of it. No depth buffer is added for
    /// this: the order `solids` already arrives in (`solid::standing`'s
    /// back-to-front sort) is what a painter's-algorithm overwrite depends on,
    /// so a wrongly ordered pair is still wrong — opaque only stops it from
    /// also blending together.
    ///
    /// `device`, `queue` and `encoder` are three different pieces of `wgpu`
    /// state a GPU pass always needs; `frame`, `camera`, `solids` and `style`
    /// are four different facts about this one — grouping any of them into a
    /// struct would be one more type to keep in step with the call sites for
    /// no fewer facts, the same call `light::collect` makes.
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        frame: Frame<'_>,
        camera: &Camera,
        solids: &[(Solid, [f32; 3])],
        style: Style,
    ) -> usize {
        if let Some(cached) = self.cached.as_ref().filter(|cached| {
            cached.camera == *camera
                && cached.rect == frame.rect
                && cached.solids == solids
                && cached.edges == style.edges
        }) {
            let bytes = Arc::clone(&cached.bytes);
            let fill_bytes = cached.fill_bytes;
            let drawn = cached.drawn;
            self.submit(
                device,
                queue,
                encoder,
                frame,
                &bytes,
                fill_bytes,
                style.opaque,
                false,
            );
            return drawn;
        }
        // Keep the caller's whole ordered list, not only the cropped subset:
        // an off-screen solid becoming visible must invalidate the bytes too.
        let source = solids.to_vec();
        // Off screen before a vertex is written. At the widest zoom most of the
        // grid is outside the viewport — the grid is grown by the widest pool's
        // reach, not cropped to the picture — and this is the difference between
        // eighteen vertices for each of those and none.
        let showing: Vec<(Solid, [f32; 3])> = solids
            .iter()
            .copied()
            .filter(|(solid, _)| on_screen(solid, camera, frame.rect))
            .collect();
        let solids = &showing[..];
        // The corners are in *viewport* pixels — `Camera::to_viewport` measures
        // from the picture's own corner — and the target is the whole surface,
        // so the world's rectangle inside it is added once, here.
        let place = |at: RealPoint| RealPoint::new(at.x + frame.rect.x as f32, at.y + frame.rect.y as f32);
        let mut bytes = Vec::with_capacity(solids.len() * 36 * VERTEX_BYTES);
        // The fills first and the whole run of them, then every outline: two
        // draws over one buffer, so the strokes land over every face rather than
        // over the faces of the same box only. A wall drawn later would otherwise
        // paint over the stroke of the one behind it.
        for (solid, colour) in solids {
            for (side, corners) in solid.faces(camera) {
                let shade = side.shade();
                let tint = [
                    colour[0] / 255.0 * shade,
                    colour[1] / 255.0 * shade,
                    colour[2] / 255.0 * shade,
                    FILL_ALPHA,
                ];
                // Two triangles from four corners in ring order.
                for index in [0, 1, 2, 0, 2, 3] {
                    write_vertex(&mut bytes, place(corners[index]), tint);
                }
            }
        }
        let fill_bytes = bytes.len();
        if style.edges {
            for (solid, _) in solids {
                for [from, to] in solid.outline(camera) {
                    write_vertex(&mut bytes, place(from), EDGE);
                    write_vertex(&mut bytes, place(to), EDGE);
                }
            }
        }
        let drawn = solids.len();
        let bytes: Arc<[u8]> = bytes.into();
        self.cached = Some(CachedVertices {
            camera: *camera,
            rect: frame.rect,
            solids: source,
            edges: style.edges,
            bytes: Arc::clone(&bytes),
            fill_bytes,
            drawn,
        });
        self.submit(
            device,
            queue,
            encoder,
            frame,
            &bytes,
            fill_bytes,
            style.opaque,
            true,
        );
        drawn
    }

    /// [`Self::render`], but with an explicit colour per *corner* of each face —
    /// `[Top, East, South]`, [`Solid::faces`]'s own order, each a
    /// [`FaceColours`] in that same order's four corners — instead of one
    /// colour per solid shaded by [`Side::shade`].
    ///
    /// For a caller whose colour *is* a measurement — a light sample, one real
    /// point per corner, rather than a kind to tell apart at a glance.
    /// `Side::shade`'s fixed table exists to make two faces of the *same* hue
    /// read as two tones; run over a value that already varies for a real
    /// reason, it would be a second, uninvited claim stacked on the first. A
    /// per-corner colour rather than [`Self::render`]'s one per face is what
    /// lets the ordinary triangle interpolation below turn four real samples
    /// into a gradient instead of a flat fill with a step at every face's edge
    /// — see [`FaceColours`]'s own doc.
    ///
    /// `style` is [`Self::render`]'s own knob, for the same reasons — see its
    /// own doc for why the argument count is not folded down further.
    #[allow(clippy::too_many_arguments)]
    pub fn render_lit(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        frame: Frame<'_>,
        camera: &Camera,
        solids: &[(Solid, [FaceColours; 3])],
        style: Style,
    ) -> usize {
        // This path shares the vertex buffer but has a different vertex format
        // payload, so the plain-colour cache cannot survive it.
        self.cached = None;
        let showing: Vec<(Solid, [FaceColours; 3])> = solids
            .iter()
            .copied()
            .filter(|(solid, _)| on_screen(solid, camera, frame.rect))
            .collect();
        let solids = &showing[..];
        let place = |at: RealPoint| RealPoint::new(at.x + frame.rect.x as f32, at.y + frame.rect.y as f32);
        let mut bytes = Vec::with_capacity(solids.len() * 36 * VERTEX_BYTES);
        for (solid, colours) in solids {
            for (face, (_side, corners)) in solid.faces(camera).into_iter().enumerate() {
                let corner_colours = colours[face];
                // Two triangles from four corners in ring order — each corner
                // keeps its own sample rather than the face's one flat tint.
                for index in [0, 1, 2, 0, 2, 3] {
                    let colour = corner_colours[index];
                    let tint = [
                        colour[0] / 255.0,
                        colour[1] / 255.0,
                        colour[2] / 255.0,
                        FILL_ALPHA,
                    ];
                    write_vertex(&mut bytes, place(corners[index]), tint);
                }
            }
        }
        let fill_bytes = bytes.len();
        if style.edges {
            for (solid, _) in solids {
                for [from, to] in solid.outline(camera) {
                    write_vertex(&mut bytes, place(from), EDGE);
                    write_vertex(&mut bytes, place(to), EDGE);
                }
            }
        }
        self.submit(
            device,
            queue,
            encoder,
            frame,
            &bytes,
            fill_bytes,
            style.opaque,
            true,
        );
        solids.len()
    }

    /// The tail both [`Self::render`] and [`Self::render_lit`] end in: upload
    /// the vertices built and draw the two pipelines over `frame`. Split out
    /// so the two colouring rules above cannot drift on how a solid actually
    /// reaches the screen.
    #[allow(clippy::too_many_arguments)]
    fn submit(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        frame: Frame<'_>,
        bytes: &[u8],
        fill_bytes: usize,
        opaque: bool,
        upload: bool,
    ) {
        if bytes.is_empty() {
            return;
        }
        let wanted = (bytes.len() / VERTEX_BYTES) as u64;
        let resized = wanted > self.capacity;
        if resized {
            // Doubling rather than the exact size: the camera moves every frame
            // and the count wobbles with it.
            self.capacity = wanted.next_power_of_two();
            self.vertices = new_vertex_buffer(device, self.capacity);
        }
        if upload || resized {
            queue.write_buffer(&self.vertices, 0, bytes);
        }
        let mut uniform_bytes = Vec::with_capacity(UNIFORM_BYTES as usize);
        for value in [frame.size.0 as f32, frame.size.1 as f32, 0.0, 0.0] {
            uniform_bytes.extend_from_slice(&value.to_le_bytes());
        }
        queue.write_buffer(&self.uniforms, 0, &uniform_bytes);

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("solids"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view:           frame.target,
                depth_slice:    None,
                resolve_target: None,
                ops:            wgpu::Operations {
                    // Loaded, never cleared: this pass draws over the frame.
                    load:  wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        // Clipped to the world's own rectangle: a solid at the edge of the
        // picture must not paint over the strip the HUD has beside it.
        pass.set_scissor_rect(frame.rect.x, frame.rect.y, frame.rect.width, frame.rect.height);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_pipeline(if opaque { &self.fills_opaque } else { &self.fills });
        pass.set_vertex_buffer(0, self.vertices.slice(..fill_bytes as u64));
        pass.draw(0..(fill_bytes / VERTEX_BYTES) as u32, 0..1);
        // `edges` false leaves nothing past `fill_bytes` — an empty slice, which
        // `wgpu` refuses outright rather than drawing zero vertices from it.
        if bytes.len() > fill_bytes {
            pass.set_pipeline(&self.edges);
            pass.set_vertex_buffer(0, self.vertices.slice(fill_bytes as u64..bytes.len() as u64));
            pass.draw(0..((bytes.len() - fill_bytes) / VERTEX_BYTES) as u32, 0..1);
        }
    }
}

/// Whether any part of a solid could land inside the world's rectangle.
///
/// The top face's four corners and the two bottom ones the vertical faces add:
/// six points, which is the solid's whole extent because the hexagon's corners
/// are a subset of them. Conservative on purpose — a box whose middle is on
/// screen and whose corners are not does not exist under this projection, but a
/// wrongly *dropped* solid is an instrument lying about what stands.
fn on_screen(solid: &Solid, camera: &Camera, rect: ViewportRect) -> bool {
    let faces = solid.faces(camera);
    let points = faces[0].1.iter().chain(&faces[1].1).chain(&faces[2].1);
    let (mut min_x, mut min_y) = (f32::MAX, f32::MAX);
    let (mut max_x, mut max_y) = (f32::MIN, f32::MIN);
    for corner in points {
        min_x = min_x.min(corner.x);
        min_y = min_y.min(corner.y);
        max_x = max_x.max(corner.x);
        max_y = max_y.max(corner.y);
    }
    max_x >= 0.0 && max_y >= 0.0 && min_x <= rect.width as f32 && min_y <= rect.height as f32
}

/// One vertex: where it is on the target, and what colour.
fn write_vertex(bytes: &mut Vec<u8>, at: RealPoint, tint: [f32; 4]) {
    for value in [at.x, at.y, tint[0], tint[1], tint[2], tint[3]] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
}

fn new_vertex_buffer(device: &wgpu::Device, vertices: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label:              Some("solids"),
        size:               vertices * VERTEX_BYTES as u64,
        usage:              wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}
