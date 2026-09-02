//! A wash over what is selected: the picked static, and the ground it stands on.
//!
//! The third way of saying "this one" — after
//! [`items::HIGHLIGHT_HUE`](crate::items::HIGHLIGHT_HUE), which replaces the
//! art's colour, and [`crate::outline`], which draws a ring round its shape.
//! What this one says is different from both, and the difference is the reason
//! it exists rather than being a fourth colour of ring: a selection is *held*,
//! not hovered, and it is about a place as much as about a thing. A ring round a
//! wall says which wall; a wash across the wall **and the floor under it** says
//! which wall and where it stands, which is what a person building or debugging
//! a map is looking at.
//!
//! # Two records, two questions
//!
//! 1. **What is the selected thing?** The silhouette mask —
//!    [`SpriteRenderer::render_mask`](crate::renderer::SpriteRenderer::render_mask)
//!    drawing [`statics::selected`](crate::statics::selected)'s quad into a mask
//!    of its own, depth-tested against the world so what it holds is what is
//!    visible.
//! 2. **What is the ground under it?** The id plane — [`crate::gbuffer`] —
//!    whose every texel names the tile the pixel drawn there came from. The
//!    ground of a tile is the land of it *or a static lying flat in it*: indoors
//!    the land is under a wooden floor and never drawn, so land alone would leave
//!    the tile unshaded in exactly the building being pointed at.
//!
//! The first cannot be answered from the second. Two walls standing on one tile
//! write the same tile, the same stance and the same height range into the
//! attachment, so a pass keyed on it alone would shade the wall beside the one
//! that was picked. That is the whole argument for the mask.
//!
//! # Why it runs after the blit
//!
//! [`Ring`](crate::outline::Ring)'s reason, unchanged: a highlight that dimmed at
//! night would stop working exactly when the picture is hardest to read. So this
//! draws on the surface, in screen pixels, over what the blit has lit.

use openshard_map::grid::Tile;

/// What one selection looks like, and where it is.
///
/// A value the caller builds each frame rather than state this module keeps: what
/// is selected is the app's answer, and a pass that remembered it would be a
/// second place for it to be true.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Selection {
    /// The tile whose ground is washed — the selected static's own tile.
    ///
    /// `None` washes the sprite alone. There is no caller for that today; it is
    /// what the type says about a selected thing that stands nowhere, rather
    /// than a coordinate of `(0, 0)` meaning two things.
    pub tile:   Option<Tile>,
    /// The wash over the selected thing's own pixels: colour, and `a` is how
    /// much of it replaces what is under it.
    pub sprite: [f32; 4],
    /// The wash over the ground it stands on. Weaker than the sprite's by
    /// default: the ground is context and the thing is the answer, and two washes
    /// of one strength read as one shape covering both.
    pub ground: [f32; 4],
}

impl Selection {
    /// The cyan the HUD already calls a selection.
    ///
    /// The same hue the Tile panel's held-tile diamond is drawn in
    /// (`shell::overlays`), on purpose: a client that said "selected" in two
    /// colours would be saying two things. Cyan rather than the ring's white
    /// because a wash *is* a colour where a ring is a shape — and nothing in UO
    /// art is cyan, which is what keeps it legible over the greys and browns of a
    /// town.
    pub const DEFAULT: Self = Self {
        tile:   None,
        // Well under half: the art has to stay readable under it. A wash at 0.6
        // is a coloured rectangle where the wall was.
        sprite: [0.0, 0.86, 1.0, 0.30],
        ground: [0.0, 0.86, 1.0, 0.18],
    };

    /// The same wash over `tile`'s ground.
    pub fn on(self, tile: Tile) -> Self {
        Self {
            tile: Some(tile),
            ..self
        }
    }
}

/// Where one wash draws, and from what.
#[derive(Clone, Copy, Debug)]
pub struct Frame<'a> {
    /// What to draw onto — the surface, after the blit has put the world there.
    pub target:           &'a wgpu::TextureView,
    /// The mask the silhouette pass filled with the selected sprite.
    ///
    /// **Its own texture and not [`outline`](crate::outline)'s.** The ring pass
    /// draws an edge round every id it finds in that one, so a selection sharing
    /// it would be ringed as well as washed — two statements, one of which
    /// nobody asked for.
    pub mask:             &'a wgpu::TextureView,
    /// What drew each world pixel. The frame's own id plane, the same one the
    /// blit lit from.
    pub ids:              &'a wgpu::TextureView,
    /// The statics pass's own instance buffer, bound a second time as storage —
    /// the same buffer `blit::Frame::face_instances` is. A `Kind::Static`
    /// pixel's id word carries a row in this, not a tile
    /// (`docs/archive/render/gbuffer.md` step 3); the ground wash resolves it the same way
    /// `blit.wgsl` does, for the same reason. See step 6.
    pub face_instances:   &'a wgpu::Buffer,
    /// The ground pass's own instance buffer, bound a second time as storage —
    /// the same buffer `blit::Frame::ground_instances` is. A `Kind::Land`
    /// pixel's `place.x`/`place.y` is an id into this, not a tile
    /// (`docs/archive/render/gbuffer.md` step 7); the ground wash resolves it the same way
    /// `blit.wgsl` does, for the same reason.
    pub ground_instances: &'a wgpu::Buffer,
    /// The size of both in texels — they are one image's size, and the pass
    /// reads them at one coordinate. Carried beside the views for the reason
    /// [`outline::Frame`](crate::outline::Frame) carries one: a view does not
    /// know its own extent.
    pub size:             (u32, u32),
    /// The rectangle of `target` the world was blitted into. The same
    /// [`ViewportRect`](crate::blit::ViewportRect) the blit was given, or the
    /// wash lands somewhere the world is not.
    pub rect:             crate::blit::ViewportRect,
}

/// Bytes of `select.wgsl`'s uniform block: four `vec4`s.
const SELECTION_BYTES: u64 = 64;

/// Turns a selection mask and the id plane into a wash on the surface.
#[derive(Debug)]
pub struct Select {
    pipeline: wgpu::RenderPipeline,
    layout:   wgpu::BindGroupLayout,
    uniforms: wgpu::Buffer,
}

impl Select {
    /// Build the pipeline for a target of `format` — the surface's, not
    /// [`WORLD_FORMAT`](crate::blit::WORLD_FORMAT): this pass draws over the
    /// blit's output. See the module docs for why.
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let uint_texture = |binding| {
            wgpu::BindGroupLayoutEntry {
                binding,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    // Both are integer records — an id and a tile — and neither is
                    // ever sampled: a place averaged with its neighbour would name a
                    // third tile nothing was drawn on, and an id averaged with one is
                    // not an id at all.
                    sample_type:    wgpu::TextureSampleType::Uint,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled:   false,
                },
                count: None,
            }
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label:   Some("select"),
            entries: &[
                uint_texture(0),
                uint_texture(1),
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
                // The statics pass's own instance data, bound a second time as
                // storage — `blit.wgsl`'s binding 9, read here for the same
                // reason: a static's `place` is an id, not a tile. Read-only,
                // like every other reader of it.
                wgpu::BindGroupLayoutEntry {
                    binding:    3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty:         wgpu::BindingType::Buffer {
                        ty:                 wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size:   None,
                    },
                    count:      None,
                },
                // The ground pass's own instance data, bound a second time as
                // storage — `blit.wgsl`'s binding 12, read here for the same
                // reason: a land pixel's `place` is an id too, since
                // `docs/archive/render/gbuffer.md` step 7. Read-only, like binding 3 above.
                wgpu::BindGroupLayoutEntry {
                    binding:    4,
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

        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label:              Some("selection"),
            size:               SELECTION_BYTES,
            usage:              wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label:  Some("select"),
            // `src/shaders/select.wesl`, compiled to plain WGSL by `build.rs`.
            source: wgpu::ShaderSource::Wgsl(include_str!(concat!(env!("OUT_DIR"), "/select.wgsl")).into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label:              Some("select"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size:     0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label:          Some("select"),
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
                    // Premultiplied, like the ring: this draws onto a finished
                    // picture, and a wash the world shows through is the whole
                    // point — see `select.wgsl`'s last paragraph.
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
            // No depth: the world's depth buffer already decided what is
            // visible, and the mask and the id plane are the record of
            // that decision.
            depth_stencil:  None,
            multisample:    wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache:          None,
        });

        Self {
            pipeline,
            layout,
            uniforms,
        }
    }

    /// Draw the wash over `frame.rect`, keeping everything else.
    ///
    /// Loads rather than clears: the blit has just written the world there, and
    /// this washes over it.
    pub fn render(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        frame: Frame<'_>,
        selection: Selection,
    ) {
        let mut bytes = Vec::with_capacity(SELECTION_BYTES as usize);
        for value in [frame.size.0 as f32, frame.size.1 as f32, 0.0, 0.0] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        let tile = selection.tile.unwrap_or(Tile::new(0, 0));
        for value in [
            u32::from(tile.x),
            u32::from(tile.y),
            u32::from(selection.tile.is_some()),
            0,
        ] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        for channel in selection.sprite.into_iter().chain(selection.ground) {
            bytes.extend_from_slice(&channel.to_le_bytes());
        }
        queue.write_buffer(&self.uniforms, 0, &bytes);

        // A bind group per call, as the blit and the ring do: both textures are
        // recreated on every resize and every zoom step, and a cached group would
        // point at one nothing is drawing into any more.
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label:   Some("select"),
            layout:  &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding:  0,
                    resource: wgpu::BindingResource::TextureView(frame.mask),
                },
                wgpu::BindGroupEntry {
                    binding:  1,
                    resource: wgpu::BindingResource::TextureView(frame.ids),
                },
                wgpu::BindGroupEntry {
                    binding:  2,
                    resource: self.uniforms.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding:  3,
                    resource: frame.face_instances.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding:  4,
                    resource: frame.ground_instances.as_entire_binding(),
                },
            ],
        });

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("select"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view:           frame.target,
                depth_slice:    None,
                resolve_target: None,
                ops:            wgpu::Operations {
                    load:  wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            multiview_mask: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });

        // An empty viewport is not a pass wgpu will take, and it is what a window
        // dragged to nothing leaves behind. The uniform is written either way, so
        // the next frame with room draws from a buffer that is current.
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
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The uniform block is a contract with a shader the compiler never sees,
    /// and the one thing in it that can be wrong without looking wrong is the
    /// "there is a tile" word: with it stuck at zero the ground is simply never
    /// washed, and the sprite still is — which reads as a deliberate design.
    #[test]
    fn a_selection_without_a_tile_says_so_rather_than_naming_nought() {
        assert_eq!(Selection::DEFAULT.tile, None);
        assert_eq!(Selection::DEFAULT.on(Tile::new(0, 0)).tile, Some(Tile::new(0, 0)));
    }

    /// The ground's wash is the weaker of the two. It is a number a person picks
    /// by looking, so what is pinned is the *relation*: the thing being pointed
    /// at is the stronger statement, and a pair that swapped would draw a floor
    /// brighter than the wall standing on it.
    #[test]
    fn the_ground_is_washed_less_than_the_thing_standing_on_it() {
        let selection = Selection::DEFAULT;
        assert!(selection.ground[3] < selection.sprite[3]);
        assert!(selection.sprite[3] < 0.5, "the art has to stay readable under it");
    }
}
