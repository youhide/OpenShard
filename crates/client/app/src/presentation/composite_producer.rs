//! The offscreen, immutable map-block producer.
//!
//! This is deliberately separate from `presentation.rs`: it owns private
//! attachments and one map-only command buffer, while the camera frame only
//! consumes completed cache entries.

use openshard_client_render::blit::ViewportRect;
use openshard_client_render::camera::TileBounds;
use openshard_client_render::composite::{
    CaptureSource, CompositeProducerJob, CompositeWorkQueue, PreparedCompositeWork,
};
use openshard_client_render::cutaway::Cutaway;
use openshard_client_render::ground;
use openshard_client_render::renderer::Target;

use crate::profile;

/// Build the ground-owned portion of one immutable map block without touching
/// the camera frame.
///
/// Only a verified `FlatGroundBlock` reaches this function. Map statics,
/// server items, mobiles, cutaway rows and UI remain live-frame owners.
pub(super) fn produce(
    resources: &crate::resources::Resources,
    window: &mut crate::window::Screen,
    composite_work: &mut CompositeWorkQueue,
    prepared: PreparedCompositeWork,
) {
    let work = prepared.work;
    let plateau = prepared.ground;
    debug_assert_eq!(plateau.block(), work.key.block);
    let job = CompositeProducerJob::for_flat_ground(work.key, plateau);
    debug_assert_eq!(job.ground(), plateau);
    debug_assert_eq!(job.source_rect().x, 0.0);
    debug_assert_eq!(job.source_rect().y, 0.0);
    debug_assert_eq!(job.source_rect().width, job.source_size().width as f32);
    debug_assert_eq!(job.source_rect().height, job.source_size().height as f32);
    let camera = job.camera();
    let (first_x, first_y) = openshard_client_render::composite::tile_origin(job.key().block);
    let owner = TileBounds {
        min_x: i32::from(first_x),
        max_x: i32::from(first_x) + openshard_map::map::BLOCK_SIZE as i32 - 1,
        min_y: i32::from(first_y),
        max_y: i32::from(first_y) + openshard_map::map::BLOCK_SIZE as i32 - 1,
    };
    let ground: Vec<_> = ground::collect_in(
        resources.map.map(),
        &camera,
        owner,
        &window.atlases.land,
        &window.atlases.texmaps,
        &Cutaway::OPEN,
    )
    .into_iter()
    .filter(|quad| quad.is_flat())
    .collect();

    let mut encoder = window
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("map block composite producer"),
        });
    let timed = profile::begin(window.gpu.as_ref(), "map composite producer", &mut encoder);
    window.composite_producer.clear(&mut encoder);
    let world = window
        .composite_producer
        .world
        .create_view(&wgpu::TextureViewDescriptor::default());
    let depth = window
        .composite_producer
        .depth
        .create_view(&wgpu::TextureViewDescriptor::default());
    let gbuffer = window.composite_producer.gbuffer.views();
    let target = Target {
        view: &world,
        depth: &depth,
        gbuffer: &gbuffer,
        width: job.source_size().width,
        height: job.source_size().height,
        projection: camera.projection(),
    };
    window
        .composite_ground
        .render(&window.device, &window.queue, &mut encoder, target, &ground);
    let (eye_x, eye_y) = camera.eye_tile();
    let source = CaptureSource {
        color: &window.composite_producer.world,
        ids: window.composite_producer.gbuffer.ids(),
        position: window.composite_producer.gbuffer.position(),
        normal: window.composite_producer.gbuffer.normal(),
        depth: &depth,
        depth_base: openshard_client_render::depth::base_for(eye_x, eye_y),
        rect: ViewportRect {
            x: 0,
            y: 0,
            width: job.source_size().width,
            height: job.source_size().height,
        },
    };
    let captured = composite_work.finish_capture(
        &window.device,
        &window.queue,
        &mut encoder,
        &mut window.composite_pass,
        &mut window.composites,
        work.key,
        source,
        plateau,
    );
    profile::end(window.gpu.as_ref(), &mut encoder, timed);
    window.queue.submit([encoder.finish()]);
    if let Some(captured) = captured {
        super::audit_captured_composite_ids(
            &window.device,
            &window.queue,
            resources.map.map(),
            captured,
            &window.composite_producer.world,
            window.composite_producer.gbuffer.ids(),
        );
    }
}
