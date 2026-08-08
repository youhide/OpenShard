//! The reference path tracer, as a gate rather than as a tool.
//!
//! `examples/boxes.rs` has run this comparison since the tracer existed, and
//! `cargo test --workspace` has never reached it: an example is a thing a person
//! runs. So the strongest statement the lighting has — *a renderer sharing no
//! arithmetic with ours, and with no notion of a tile anywhere in it, agrees
//! about every interior pixel* — was one nobody would notice going false. This
//! file is that statement under `cargo test`.
//!
//! ```sh
//! cargo test -p openshard-client-render --test traced -- --nocapture
//! ```
//!
//! A machine with no GPU adapter skips it, which is the same bargain the rest of
//! the GPU suite makes. The tracer itself needs no GPU; what needs one is the
//! frame it is compared against.
//!
//! # Why it reaches into `examples/`
//!
//! The judging — what counts as a disagreement, and which of the four kinds it
//! is — is `examples/oracle/pathtrace.rs`, shared with the tool by `#[path]`
//! rather than copied. That module cannot be a library: it names
//! `openshard-client-pathtrace`, which is a **dev-dependency** of this crate
//! precisely so the shipped renderer cannot reach the thing that checks it, and
//! code naming a dev-dependency can live in `examples/` or `tests/` and nowhere
//! else.
//!
//! Given that, the choice was one copy reached by an unusual path or two copies
//! of the rule. Two copies is how a gate ends up green about a rule the tool no
//! longer applies — and the rule is exactly where a defect would hide, because
//! every one of its four splits is a decision about what *not* to report.
//!
//! What is **not** shared is the pipeline boilerplate below: building a scene,
//! rendering it, reading it back. Every GPU fixture in this crate has its own,
//! and a second one is a nuisance rather than a hazard — it cannot make a
//! disagreement disappear, only fail to produce one, which the non-triviality
//! assertions at the end are there to catch.

// Reached from `tests/`, so most of it is unused here — the slab oracle and the
// crosshair belong to the tool.
#[allow(dead_code)]
#[path = "../examples/oracle/mod.rs"]
mod oracle;

use openshard_client_pathtrace::trace as pt_trace;
use openshard_client_render::camera::{Camera, TileBounds, WorldSpot, Zoom, project_exact};
use openshard_client_render::cutaway::Cutaway;
use openshard_client_render::debug::View;
use openshard_client_render::depth;
use openshard_client_render::geometry::Vec2;
use openshard_client_render::light::{Light, Lighting, NIGHT};
use openshard_client_render::mesh_face::{MeshFaceRow, MeshFaceVertex};
use openshard_client_render::occlusion::{Builder, OwnerId};
use openshard_client_render::place::Stance;
use openshard_client_render::renderer::{self, GroundRenderer, MeshFaceRenderer, Target};
use openshard_protocol::wire::Graphic;
use openshard_uofiles::tiledata::{StaticTile, TileFlags};

use oracle::boxes::{BoxSpec, box_mesh, box_owner};

/// The frame the gate is measured over. Square, and large enough that a box's
/// own face is thousands of pixels rather than dozens: the comparison's whole
/// value is that it is a *picture* and not a point query.
const SIDE: u32 = 512;

/// A GPU to draw with, or `None` where there is none.
fn gpu() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::default();
    let adapter =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).ok()?;
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()
}

/// `examples/boxes.rs`'s own `line` scene: two whole-tile boxes side by side due
/// east, four `z` units tall.
///
/// The same scene and the same flame the tool's own recorded numbers are from
/// (`docs/lighting_reference.md`), so a person reading a failure here can run
/// the tool on the same thing and get the picture. Two boxes rather than one
/// because a single box cannot produce the case that matters most: an occluder
/// that is not the one the fragment is standing on.
fn line_scene() -> Vec<BoxSpec> {
    let h = 4.0;
    vec![
        BoxSpec {
            tile: (100, 100),
            min: (100.0, 100.0, 0.0),
            max: (101.0, 101.0, h),
        },
        BoxSpec {
            tile: (101, 100),
            min: (101.0, 100.0, 0.0),
            max: (102.0, 101.0, h),
        },
    ]
}

/// Up and to the boxes' `+x`, `-y` side, above them — the tool's own default for
/// this scene, picked there by looking at a rendered frame.
fn flame() -> WorldSpot {
    WorldSpot {
        x: 102.5,
        y: 98.5,
        z: 6.0,
    }
}

const FLAME_RADIUS: f32 = 8.0;

#[test]
fn the_frame_and_the_path_tracer_agree_about_every_interior_pixel() {
    let Some((device, queue)) = gpu() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let boxes = line_scene();

    let bounds = TileBounds {
        min_x: 95,
        max_x: 107,
        min_y: 95,
        max_y: 106,
    };
    // NO_SHOOT so a box occludes light at all (`occlusion::opacity`'s own doc: a
    // graphic's own flags decide it, not the shape). `height` here is only what
    // `depth::static_priority_z` reads off it; the occluder's real span comes
    // from `add_raw`'s own `space`.
    let cube_tile = StaticTile {
        flags: TileFlags::new(TileFlags::NO_SHOOT),
        height: 1,
        ..StaticTile::default()
    };
    let mut builder = Builder::new(bounds);
    for (index, b) in boxes.iter().enumerate() {
        builder.add_raw(b.tile.0, b.tile.1, b.solid(), box_owner(index, b));
    }
    let occlusion = builder.finish(&Cutaway::OPEN);
    let owners: Vec<OwnerId> = boxes
        .iter()
        .enumerate()
        .map(|(index, b)| {
            let owner = box_owner(index, b);
            let id = occlusion.owner_at(i32::from(b.tile.0), i32::from(b.tile.1), owner.z, owner.graphic);
            assert_ne!(
                id,
                OwnerId::NONE,
                "box {index} is not in the grid this test just built — the comparison would then be \
                 measuring a scene with one box missing, and would pass for it"
            );
            id
        })
        .collect();

    // Three notches is the top of `camera::LADDER` — 4:1 — where a whole-tile
    // box fills a 512-pixel canvas comfortably.
    let (centre_x, centre_y) = (100, 100);
    let mut camera = Camera::new(
        openshard_protocol::world::Point::new(centre_x as u16, centre_y as u16, 0),
        SIDE,
        SIDE,
    );
    camera.zoom_about(
        (SIDE / 2) as i32,
        (SIDE / 2) as i32,
        Zoom::ONE.scale_up().scale_up().scale_up(),
    );
    let projection = camera.projection();
    // Where a world position lands in this frame, in real pixels. The tracer's
    // camera is *measured* through this closure and never restates it — see
    // `oracle::pathtrace::Mirror::of`.
    let to_pixel = |at: WorldSpot| -> (f64, f64) {
        let screen = camera.to_view_exact(project_exact(at));
        (
            f64::from((screen.x - projection.origin.x) * projection.scale + SIDE as f32 * 0.5),
            f64::from((screen.y - projection.origin.y) * projection.scale + SIDE as f32 * 0.5),
        )
    };

    let base_tile = depth::base_for(centre_x, centre_y);
    let mut rows: Vec<MeshFaceRow> = Vec::new();
    let mut vertices: Vec<MeshFaceVertex> = Vec::new();
    // Which row each box's each face was pushed as, kept while it is pushed
    // rather than re-derived from `rows.len()` arithmetic later: it is what the
    // comparison matches the rendered `place` attachment's own id against, so
    // "this pixel is box 1's south face" is the renderer's answer and not this
    // test's guess about the order it built its own list in.
    let mut face_rows: Vec<(usize, Stance, u32)> = Vec::new();
    for (box_index, b) in boxes.iter().enumerate() {
        let solid = b.solid();
        let d = depth::Order {
            tile: i32::from(b.tile.0) + i32::from(b.tile.1),
            priority_z: depth::static_priority_z(solid.min.z.round() as i8, &cube_tile),
        }
        .to_depth(base_tile);
        for face in box_mesh(solid).faces() {
            let id = rows.len() as u32;
            let stance = Stance::of_normal(face.normal).expect("a box face's own axis-aligned normal");
            face_rows.push((box_index, stance, id));
            rows.push(MeshFaceRow {
                tile: (b.tile.0, b.tile.1),
                stance,
                owner: u32::from(owners[box_index].raw()),
            });
            for corner in face.fan() {
                vertices.push(MeshFaceVertex {
                    screen: camera.to_view_exact(project_exact(corner)),
                    world: [corner.x as f32, corner.y as f32, corner.z as f32],
                    depth: d,
                    id,
                    tile: [f32::from(b.tile.0), f32::from(b.tile.1)],
                });
            }
        }
    }

    let format = wgpu::TextureFormat::Rgba8Unorm;
    let world = openshard_client_render::blit::world_texture(&device, SIDE, SIDE);
    let world_view = world.create_view(&wgpu::TextureViewDescriptor::default());
    let place_tex = openshard_client_render::place::texture(&device, SIDE, SIDE);
    let place_view = place_tex.create_view(&wgpu::TextureViewDescriptor::default());
    let depth_tex = renderer::depth_texture(&device, SIDE, SIDE);
    let depth_view = depth_tex.create_view(&wgpu::TextureViewDescriptor::default());

    // A floor for a shadow to fall on: one flat synthetic land tile, repeated
    // over the same bounds the occlusion grid covers.
    const FLOOR: Graphic = Graphic(3);
    let floor_pixel = openshard_uofiles::color::Color16((20 << 10) | (20 << 5) | 20);
    let floor_image = openshard_uofiles::image::Image::new(
        openshard_uofiles::art::LAND_TILE_SIZE,
        openshard_uofiles::art::LAND_TILE_SIZE,
        vec![floor_pixel; usize::from(openshard_uofiles::art::LAND_TILE_SIZE).pow(2)],
    );
    let blocks = (bounds.max_x as u32).div_ceil(openshard_uofiles::map::BLOCK_SIZE) + 1;
    let synthetic_map = openshard_uofiles::map::Map::from_blocks(blocks, blocks, |_x, _y| {
        openshard_uofiles::map::LandCell { tile: FLOOR.0, z: 0 }
    });
    let land = openshard_client_render::atlas::LandAtlas::pack([(FLOOR, floor_image)])
        .expect("one flat tile always fits");
    let texmaps = openshard_client_render::atlas::TexmapAtlas::pack([]).expect("nothing always fits");
    let ground_quads =
        openshard_client_render::ground::collect(&synthetic_map, &camera, &land, &texmaps, &Cutaway::OPEN);

    let mut ground_pass = GroundRenderer::new(&device, &queue, format, &land, &texmaps);
    let mut mesh_pass = MeshFaceRenderer::new(&device);
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    let target = Target {
        place: &place_view,
        view: &world_view,
        depth: &depth_view,
        width: SIDE,
        height: SIDE,
        projection,
    };
    ground_pass.render(&device, &queue, &mut encoder, target, &ground_quads);
    mesh_pass.render(&device, &queue, &mut encoder, target, &vertices, &rows);
    queue.submit([encoder.finish()]);

    // What the world passes left on each pixel: which surface owns it, and where
    // in the world that surface's own fragment is.
    let drawn = oracle::read_place(&device, &queue, &place_tex, SIDE, SIDE);

    let at = flame();
    let lighting = Lighting {
        ambient: NIGHT,
        lights: vec![Light {
            at: Vec2::new(at.x as f32, at.y as f32),
            z: at.z as f32,
            radius: FLAME_RADIUS,
            color: [1.0, 1.0, 1.0],
            intensity: 1.0,
            beam: None,
        }],
        occlusion,
        sun: None,
        view: View::Shadow,
    };

    let surface = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("surface"),
        size: wgpu::Extent3d {
            width: SIDE,
            height: SIDE,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let surface_view = surface.create_view(&wgpu::TextureViewDescriptor::default());
    let dummy_instances = openshard_client_render::blit::dummy_instances(&device);
    let mut blit = openshard_client_render::blit::Blit::new(&device, format);
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    blit.render(
        &device,
        &queue,
        &mut encoder,
        openshard_client_render::blit::Frame {
            target: &surface_view,
            world: &world_view,
            place: &place_view,
            face_instances: &dummy_instances,
            mobile_instances: &dummy_instances,
            mesh_instances: mesh_pass.rows_buffer(),
            ground_instances: ground_pass.instances_buffer(),
            zoom: Zoom::ONE,
            rect: openshard_client_render::blit::ViewportRect {
                x: 0,
                y: 0,
                width: SIDE,
                height: SIDE,
            },
        },
        &lighting,
    );
    queue.submit([encoder.finish()]);
    let shadow = oracle::read_surface(&device, &queue, &surface, SIDE, SIDE);

    let mirror = oracle::pathtrace::Mirror::of(&boxes, at, f64::from(FLAME_RADIUS), &to_pixel);
    let verdict = oracle::pathtrace::compare(
        &mirror.render(pt_trace::Brdf::Flat, SIDE, SIDE),
        &mirror.render(pt_trace::Brdf::Lambert, SIDE, SIDE),
        oracle::pathtrace::Frame {
            width: SIDE,
            height: SIDE,
            drawn: &drawn,
            shadow: &shadow,
            face_rows: &face_rows,
        },
    );
    eprint!("{}", verdict.report());

    // The scene has to be one where the answer could have been wrong. All three
    // of these are the same guard from different sides: a frame that drew
    // nothing, a torch that reached everything, and a torch that reached nothing
    // would each pass the assertion that matters while measuring no shadow at
    // all.
    assert!(
        verdict.compared > 200_000,
        "only {} of {} pixels were compared — a detector that compares nothing reads exactly like a \
         detector that found nothing",
        verdict.compared,
        SIDE * SIDE,
    );
    let lit = verdict.traced_lit.iter().flatten().filter(|lit| **lit).count();
    let dark = verdict.traced_lit.iter().flatten().filter(|lit| !**lit).count();
    assert!(
        lit > 10_000 && dark > 10_000,
        "the tracer saw {lit} lit and {dark} shadowed pixels: a scene that is all one or the other \
         agrees with anything"
    );
    assert!(
        verdict.back_facing > 1_000,
        "only {} back-facing pixels: the comparison is not reaching the surfaces the walk's own \
         exemption decides, which are the ones it is most worth reaching",
        verdict.back_facing,
    );

    // And the gate. Every pixel where the two renderers agree what surface is
    // there, and where neither picture has an edge running through the pixel's
    // own neighbourhood, they agree about whether the flame reaches it — against
    // a renderer that shares no arithmetic with ours and has no notion of a tile.
    assert_eq!(
        verdict.interior,
        0,
        "the path tracer and the frame disagree about {} pixels that no edge and no surface \
         disagreement explains\n{}",
        verdict.interior,
        verdict.report(),
    );
}
