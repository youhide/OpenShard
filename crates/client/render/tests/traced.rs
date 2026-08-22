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
use openshard_client_render::camera::{Camera, RealPixel, TileBounds, WorldSpot, Zoom, project_exact};
use openshard_client_render::cutaway::Cutaway;
use openshard_client_render::debug::View;
use openshard_client_render::depth;
use openshard_client_render::geometry::Vec2;
use openshard_client_render::light::{Light, Lighting, NIGHT};
use openshard_client_render::mesh_face::{MeshFaceRow, MeshFaceVertex};
use openshard_client_render::occlusion::{Builder, Part, SolidId};
use openshard_client_render::place::Stance;
use openshard_client_render::renderer::{self, GroundRenderer, MeshFaceRenderer, Target};
use openshard_map::grid::BlockExtent;
use openshard_protocol::wire::Graphic;
use openshard_uofiles::tiledata::{StaticTile, TileFlags};

use oracle::boxes::{BoxSpec, box_mesh, box_owner};

/// The frame the gate is measured over. Square, and large enough that a box's
/// own face is thousands of pixels rather than dozens: the comparison's whole
/// value is that it is a *picture* and not a point query.
const SIDE: u32 = 512;
const TRACE_SIZE: pt_trace::ImageSize = pt_trace::ImageSize::new(SIDE, SIDE);

/// A GPU to draw with, or `None` where there is none.
fn gpu() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::default();
    let adapter =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).ok()?;
    // `Gbuffer` needs this exact floating-point render target. A downlevel
    // adapter that cannot create it is not a GPU this parity gate can exercise;
    // report that by returning `None`, like the other frame suites, instead of
    // failing later inside `Gbuffer::new`.
    if !adapter
        .get_texture_format_features(openshard_client_render::gbuffer::POSITION_FORMAT)
        .allowed_usages
        .contains(wgpu::TextureUsages::RENDER_ATTACHMENT)
    {
        return None;
    }
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        required_limits: openshard_client_render::gbuffer::required_limits(),
        ..Default::default()
    }))
    .ok()
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
            graphic: 0,
        },
        BoxSpec {
            tile: (101, 100),
            min: (101.0, 100.0, 0.0),
            max: (102.0, 101.0, h),
            // A second graphic, so these stay two primitives: the point of this
            // scene is an occluder that is *not* the one the fragment stands on,
            // and one graphic would make `occlusion::merge` fold the two into a
            // single box — which is [`wall_run_scene`]'s subject, not this one.
            graphic: 1,
        },
    ]
}

/// Three climbable flights side by side, as nine boxes — `examples/synthetic_
/// stair`'s own `OPENSHARD_STAIR_RUN=3` scene, written out rather than built
/// through `facing::Prism` so that the tracer and the frame are handed the
/// **same nine AABBs** and nothing between them can differ.
///
/// The geometry, from that tool's own printout: each flight owns one tile, its
/// three treads divide that tile in thirds along `y` (a north climb), and each
/// tread is a body from the static's base at `z 0` up to its own height of 1, 3
/// and 5. So the three flights' treads abut **exactly** across `x = 101` and
/// `x = 102` — three tiles of one continuous landing at `z 5`, three of one at
/// `z 3`, three of one at `z 1`.
///
/// **Why this scene and not one more pair of cubes.** Every box here has a
/// neighbour it shares a whole face with, at the same height, belonging to a
/// *different static* — the shape `docs/occluders.md` is about, and the one no
/// scene compared against the tracer has ever had. `line_scene`'s two boxes abut
/// too, but they are one storey of one height: nothing in them can pose the
/// question of a surface that is continuous across a boundary while its
/// primitives are not.
fn stair_scene() -> Vec<BoxSpec> {
    let third = 1.0 / 3.0;
    let mut boxes = Vec::new();
    for flight in 0..3 {
        let x = 100.0 + f64::from(flight);
        // Tread `n` covers the strip `n` thirds from the tile's own far edge and
        // stands `1 + 2n` tall — `Prism::new(North, &[1, 3, 5])`'s own profile.
        // **Top tread first within a flight, and that is not a detail.** The
        // three share a tile and therefore a `depth::Order`, and a tie there goes
        // to whichever was pushed later, so the last one painted wins. `box_mesh`
        // gives every box a full-height `+y` face knowing nothing about what
        // abuts it, so the part of a tread's riser below the tread in front of it
        // is interior to the union — real geometry no camera can see. Painting
        // the near treads last is what buries it.
        //
        // In climb order instead, which is how this scene was first written, that
        // buried riser is drawn *over* the tread in front and the tracer reports
        // it: **7,016 pixels** of "the frame draws box N's south face, the tracer
        // sees body N−1", none of them on a silhouette. The tracer was right and
        // the order was wrong. `examples/boxes.rs`'s own `scene_stair` carries the
        // same rule and found the same thing first.
        for (tread, height) in [1.0, 3.0, 5.0].into_iter().enumerate().rev() {
            let near = 100.0 + (2 - tread) as f64 * third;
            boxes.push(BoxSpec {
                tile: (100 + flight as u16, 100),
                min: (x, near, 0.0),
                max: (x + 1.0, near + third, height),
                // Every tread of every flight is its own static, so the landings
                // stay three primitives each and this scene keeps asking what it
                // has always asked: whether the walk is right about nine abutting
                // boxes. A run of one graphic is [`wall_run_scene`]'s question.
                graphic: boxes.len() as u16,
            });
        }
    }
    boxes
}

/// The tiles a run of wall stands on — three, side by side due east on one row.
const WALL_RUN: std::ops::Range<u16> = 100..103;

/// **A run of one wall, which is the one shape every scene in this file was
/// unable to be** — whole-tile boxes standing in a row, four `z` tall, each
/// naming the graphic `graphic` gives it.
///
/// `&|_| 7` is a run of *one* static: one `Owner` over three tiles, which is
/// exactly what `occlusion::merge` folds into a single primitive whose box is
/// the union of the three. `&|x| x` is the same geometry as three statics, which
/// merges nothing at all. The two are the twin `tests/lighting.rs`'s
/// `a_merged_run_answers_every_ray_the_way_its_own_pieces_did` is built on, said
/// here in the geometry the *frame* and the reference tracer can both be handed.
///
/// **Why the file needed it.** `box_owner` used to key a box by its place in the
/// list, so every scene here had a graphic per box and nothing could merge under
/// any circumstances — which is how S3b landed with its ten CPU gates red under
/// injection and `tests/traced.rs`, `tests/frame.rs` and `tests/pictures.rs` all
/// green. The only non-circular arbiter this track has had never saw a merged
/// frame. `docs/occluders.md` records that as the merge's own blind spot, with
/// this fixture as the recipe.
///
/// Whole tiles and one height, so the union of the three boxes *is* their
/// componentwise union — a convex slab. That is what makes the comparison fair
/// rather than lenient: the tracer is handed three boxes and the walk one, and if
/// the merge invented or lost any volume at all the two pictures are about
/// different geometry.
fn wall_run_scene(graphic: &dyn Fn(u16) -> u16) -> Vec<BoxSpec> {
    let h = 4.0;
    WALL_RUN
        .map(|x| BoxSpec {
            tile: (x, 100),
            min: (f64::from(x), 100.0, 0.0),
            max: (f64::from(x) + 1.0, 101.0, h),
            graphic: graphic(x),
        })
        .collect()
}

/// Off the run's east end, half a tile clear of its south face and two `z` above
/// its top.
///
/// **Off the end so that rays run *along* the length of the merged box** — the
/// arrangement a union that grew too far or not far enough answers differently
/// from three boxes, and the same third flame the CPU twin picked for the same
/// reason. Above the top so that the ground either side of the run is lit and
/// only the strip behind it is not: a frame that is all shadow agrees with
/// anything.
///
/// **And clear of the south face's own plane, which is not cosmetic.**
/// [`box_mesh`] draws a box's top, `+x` and `+y` faces, so a flame *north* of
/// `y = 101` leaves the drawn south faces turned away from it — and a fragment
/// turned away is where the merge's own exemption reaches further than the
/// reference's. `a_merged_run_is_exempt_from_itself_only_where_the_cosine_is_
/// already_nothing` is that case, measured, on this very scene; here the light is
/// on the same side as every face it is compared over, so what is being compared
/// is the merged box's *geometry*.
fn wall_run_flame() -> WorldSpot {
    WorldSpot {
        x: 103.5,
        y: 101.5,
        z: 6.0,
    }
}

/// The same run lit from **inside the row's own line**, where every drawn south
/// face is turned away from the flame — see
/// [`a_merged_run_is_exempt_from_itself_only_where_the_cosine_is_already_nothing`].
fn wall_run_grazing_flame() -> WorldSpot {
    WorldSpot {
        x: 103.5,
        y: 100.5,
        z: 6.0,
    }
}

/// Which way a box face drawn by [`box_mesh`] looks: the axis of its own normal,
/// and the sign along it.
///
/// Only the three faces that tool draws occur — a lid and the two the isometric
/// camera can see — and a scene that grew a fourth should say so rather than be
/// guessed at.
fn face_normal(stance: Stance) -> (usize, f64) {
    match stance {
        Stance::Flat => (2, 1.0),
        Stance::FaceSouth => (1, 1.0),
        Stance::FaceEast => (0, 1.0),
        other => panic!("a box's face came back as {other:?}"),
    }
}

/// Where the two frames of a twin differ, pixel by pixel.
///
/// A whole `RGBA8` texel compared at once and no tolerance anywhere: an opaque
/// primitive stops a ray or does not, so each of the eight rays lands the same
/// way on both sides and a partly lit fragment comes out the same eighth.
fn moved_pixels(one: &Rendered, other: &Rendered) -> Vec<usize> {
    (0..(SIDE * SIDE) as usize)
        .filter(|pixel| one.surface[pixel * 4..pixel * 4 + 4] != other.surface[pixel * 4..pixel * 4 + 4])
        .collect()
}

/// One geometry stated twice — as a run of one graphic, which merges into a
/// single primitive, and as the same three boxes under three graphics, which
/// merges into nothing — drawn into two frames that may differ in nothing else.
///
/// One [`Shot`] said once and handed to both, since a second literal is where a
/// camera or a flame quietly stops being the same one.
fn wall_run_twin(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    at: WorldSpot,
    view: View,
) -> (Rendered, Rendered) {
    fn shot(boxes: &[BoxSpec], at: WorldSpot, view: View) -> Shot<'_> {
        Shot {
            boxes,
            flame: at,
            radius: 8.0,
            intensity: 1.0,
            ambient: NIGHT,
            view,
            // Three tiles across, the same footprint as the run of flights and
            // the same argument: at 4:1 the far end of the run would be off the
            // picture, and the far end is what a wrong union moves.
            zoom: 2,
            // **The shipped sphere, and it is the sensitive choice.** A soft edge
            // is a grey level a pixel — eighths of a flame — where a hard shadow
            // is one bit, so a union off by a fraction of a tile shows up here as
            // a moved grey rather than as nothing at all.
            flame_radius: openshard_client_render::light::FLAME_RADIUS,
        }
    }
    let one = wall_run_scene(&|_| 7);
    let three = wall_run_scene(&|x| x);
    let merged = render(device, queue, shot(&one, at, view));
    let pieces = render(device, queue, shot(&three, at, view));
    assert_eq!(merged.primitives, 1, "the run of one graphic did not merge");
    assert_eq!(
        pieces.primitives,
        WALL_RUN.len(),
        "the run of three graphics merged, so this twin is not a twin",
    );
    (merged, pieces)
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

/// One frame of a scene of boxes, and everything a comparison needs to read it.
///
/// Two tests draw a frame here — the shadow gate and the brightness gate — and
/// they differ in the scene, the ambient and which view they read back. Building
/// the pipeline twice would be two fixtures that can drift apart in every way
/// that is not under test; the camera in particular, which the reference tracer
/// *measures* through this fixture's own projection.
struct Rendered {
    /// What the world passes left on each pixel: the surface, and where its
    /// fragment is.
    drawn: Vec<oracle::Drawn>,
    /// The requested [`View`], read back, `RGBA8`.
    surface: Vec<u8>,
    /// The art itself, before any light — what the albedo of a comparison has to
    /// be taken from.
    world: Vec<u8>,
    /// Which box and stance each mesh-face row is, recorded as the rows were
    /// pushed rather than re-derived.
    face_rows: Vec<(usize, Stance, u32)>,
    /// Which primitive of the grid each box's own fragments are met against —
    /// `Occlusion::id_of`'s answer, box by box.
    ///
    /// Out here because after S3b's merge **two boxes can name one primitive**,
    /// and a gate about the merge has to be able to say that they do without
    /// building a second grid to look at: a fixture that asserted on its own
    /// second copy of the build would be asserting about the copy.
    solids: Vec<SolidId>,
    /// How many primitives the frame's grid came out holding, which is the
    /// merge's own count for the scene above.
    primitives: usize,
    /// The frame's own world-to-pixel map. Boxed because the tracer's camera is
    /// recovered from it as a black box and this fixture is what owns the
    /// camera it closes over.
    to_pixel: Box<dyn Fn(WorldSpot) -> (f64, f64)>,
}

/// A frame to draw: the scene, the light on it, and how close the camera is.
///
/// The zoom is a field and not a constant because it is **what decides how much
/// of the world a 512-pixel canvas holds**, and the two gates want opposite
/// things from that. The shadow gate wants a box's own face to be thousands of
/// pixels, which is 4:1. The brightness gate wants the flame's whole pool —
/// bright centre *and* dark rim — inside the frame, and at 4:1 a canvas is three
/// tiles across, so every pixel of it sits near the middle of an eight-tile pool
/// and the picture is one flat brightness. That version of the test agreed with
/// the tracer and would have agreed with almost any falloff curve; its own
/// non-triviality assertion is what caught it.
struct Shot<'a> {
    boxes: &'a [BoxSpec],
    flame: WorldSpot,
    /// The flame's reach, in tiles — `light::Light::radius`.
    radius: f32,
    /// How brightly it burns — `light::Light::intensity`.
    ///
    /// A field since phase 3, and the reason is the brightness gate: with a
    /// cosine in the term a pool's *bright* half is a disc under the flame rather
    /// than most of the canvas, and how wide that disc is is what decides whether
    /// a comparison is asking the falloff curve about its whole range or about
    /// its tail. A shadow comparison does not care and passes `1.0`.
    intensity: f32,
    ambient: openshard_client_render::light::Ambient,
    view: View,
    /// Notches up `camera::LADDER` from 1:1.
    zoom: u32,
    /// How big the flame is — `light::Lighting::flame_radius`, and the reason
    /// that is a field rather than a constant.
    ///
    /// [`light::FLAME_RADIUS`](openshard_client_render::light::FLAME_RADIUS) is
    /// what a frame draws, and the two gates that judge a *soft* edge pass it.
    /// **Zero is a point, and it is what a gate judging the walk itself wants**:
    /// eight rays at a sphere are an estimate, the reference's sixty-four paths
    /// are a better one, and their difference is noise that a binary lit/shadowed
    /// comparison reports as disagreement. At zero there is no estimate on either
    /// side and the two renderers either agree exactly or do not.
    flame_radius: f32,
}

/// Draw a [`Shot`] and read the frame back.
fn render(device: &wgpu::Device, queue: &wgpu::Queue, shot: Shot<'_>) -> Rendered {
    let Shot {
        boxes,
        flame,
        radius,
        intensity,
        ambient,
        view,
        zoom: zoom_notches,
        flame_radius,
    } = shot;
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
    for b in boxes {
        builder.add_raw(b.tile.0, b.tile.1, b.solid(), box_owner(b));
    }
    let occlusion = builder.finish(&Cutaway::OPEN);
    // Which *solid* of the grid each box is — `add_raw` pushes exactly one per
    // box, so `Part::ONLY` names it. `docs/lighting_rebuild.md` phase 4: this is
    // the number a fragment carries and the walk compares, and it was the box's
    // `OwnerId` until the phase found one level of identity too coarse for a
    // flight of steps.
    let solids: Vec<SolidId> = boxes
        .iter()
        .enumerate()
        .map(|(index, b)| {
            occlusion
                .id_of(i32::from(b.tile.0), i32::from(b.tile.1), box_owner(b), Part::ONLY)
                .unwrap_or_else(|| {
                    panic!(
                        "box {index} is not in the grid this test just built — the comparison would \
                         then be measuring a scene with one box missing, and would pass for it"
                    )
                })
        })
        .collect();

    let (centre_x, centre_y) = (100, 100);
    let mut camera = Camera::new(
        openshard_protocol::world::Point::new(centre_x as u16, centre_y as u16, 0),
        SIDE,
        SIDE,
    );
    let mut zoom = Zoom::ONE;
    for _ in 0..zoom_notches {
        zoom = zoom.scale_up();
    }
    camera.zoom_about(RealPixel::new((SIDE / 2) as i32, (SIDE / 2) as i32), zoom);
    let projection = camera.projection();
    // Where a world position lands in this frame, in real pixels. The tracer's
    // camera is *measured* through this closure and never restates it — see
    // `oracle::pathtrace::Mirror::of`. Its own copy of the camera, so the
    // fixture can hand it back to a caller that outlives this function.
    let mapped = camera;
    let to_pixel = move |at: WorldSpot| -> (f64, f64) {
        let screen = mapped.to_view_exact(project_exact(at));
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
                solid: solids[box_index].raw(),
            });
            for corner in face.fan() {
                vertices.push(MeshFaceVertex {
                    screen: camera.to_view_exact(project_exact(corner)),
                    world: [corner.x as f32, corner.y as f32, corner.z as f32],
                    depth: d,
                    id,
                    tile: [f32::from(b.tile.0), f32::from(b.tile.1)],
                    normal: face.normal,
                    // The same authored value `oracle::pathtrace::Albedos::
                    // INVENTED.body` is, so a brightness gate that reads this
                    // back with `oracle::body_albedo` is measuring a number
                    // both sides were actually given, not a coincidence.
                    colour: oracle::pathtrace::Albedos::INVENTED.body.map(|c| c as f32),
                });
            }
        }
    }

    let format = wgpu::TextureFormat::Rgba8Unorm;
    let world = openshard_client_render::blit::world_texture(device, SIDE, SIDE);
    let world_view = world.create_view(&wgpu::TextureViewDescriptor::default());
    let gbuffer = openshard_client_render::gbuffer::Gbuffer::new(device, SIDE, SIDE);
    let gbuffer_views = gbuffer.views();
    let depth_tex = renderer::depth_texture(device, SIDE, SIDE);
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
    let blocks = (bounds.max_x as u32).div_ceil(openshard_map::map::BLOCK_SIZE) + 1;
    let synthetic_map = openshard_map::map::Map::from_blocks(
        BlockExtent {
            wide: blocks,
            down: blocks,
        },
        |_x, _y| openshard_map::map::LandCell {
            tile: openshard_map::map::LandTile(FLOOR.0),
            z: 0,
        },
    );
    let land = openshard_client_render::atlas::LandAtlas::pack([(FLOOR, floor_image)])
        .expect("one flat tile always fits");
    let texmaps = openshard_client_render::atlas::TexmapAtlas::pack([]).expect("nothing always fits");
    let ground_quads =
        openshard_client_render::ground::collect(&synthetic_map, &camera, &land, &texmaps, &Cutaway::OPEN);

    let mut ground_pass = GroundRenderer::new(device, queue, format, &land, &texmaps);
    let mut mesh_pass = MeshFaceRenderer::new(device);
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    let target = Target {
        gbuffer: &gbuffer_views,
        view: &world_view,
        depth: &depth_view,
        width: SIDE,
        height: SIDE,
        projection,
    };
    ground_pass.render(device, queue, &mut encoder, target, &ground_quads);
    mesh_pass.render(device, queue, &mut encoder, target, &vertices, &rows);
    queue.submit([encoder.finish()]);

    // What the world passes left on each pixel: which surface owns it, and where
    // in the world that surface's own fragment is.
    let drawn = oracle::read_gbuffer(device, queue, &gbuffer, SIDE, SIDE);

    // Read off before the grid is handed to the frame, since the frame owns it
    // from here on.
    let primitives = occlusion.solids().len();
    let lighting = Lighting {
        ambient,
        lights: vec![Light {
            at: Vec2::new(flame.x as f32, flame.y as f32),
            z: flame.z as f32,
            radius,
            color: [1.0, 1.0, 1.0],
            intensity,
            beam: None,
        }],
        occlusion,
        sun: None,
        view,
        flame_radius,
        // The default, which is the count the shader's own header will carry:
        // this gate is about where the rays go, and a fixture that cast a
        // different number than the client does would be measuring a different
        // estimate.
        shadow_rays: openshard_client_render::light::ShadowRays::DEFAULT,
        dead: false,
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
    let dummy_instances = openshard_client_render::blit::dummy_instances(device);
    let mut blit = openshard_client_render::blit::Blit::new(device, format);
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    blit.render(
        device,
        queue,
        &mut encoder,
        openshard_client_render::blit::Frame {
            target: &surface_view,
            world: &world_view,
            gbuffer: &gbuffer_views,
            face_instances: &dummy_instances,
            item_instances: &dummy_instances,
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

    Rendered {
        drawn,
        surface: oracle::read_surface(device, queue, &surface, SIDE, SIDE),
        world: oracle::read_surface(device, queue, &world, SIDE, SIDE),
        face_rows,
        solids,
        primitives,
        to_pixel: Box::new(to_pixel),
    }
}

#[test]
fn the_frame_and_the_path_tracer_agree_about_every_interior_pixel() {
    let Some((device, queue)) = gpu() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let boxes = line_scene();
    let at = flame();
    let frame = render(
        &device,
        &queue,
        Shot {
            boxes: &boxes,
            flame: at,
            radius: FLAME_RADIUS,
            // Visibility, so brightness is not what is being read: any intensity
            // that lights the scene at all answers the same question.
            intensity: 1.0,
            ambient: NIGHT,
            view: View::Shadow,
            // Three notches is the top of `camera::LADDER` — 4:1 — where a
            // whole-tile box fills a 512-pixel canvas comfortably.
            zoom: 3,
            // The shipped sphere: this gate carries the penumbra assertions
            // below, and they are the ones phase 5 is judged by.
            flame_radius: openshard_client_render::light::FLAME_RADIUS,
        },
    );

    // `Albedos::INVENTED`, and it says so: this comparison is about *visibility*
    // — which pixels the flame reaches — and nothing in it reads a colour. The
    // brightness comparison that does is
    // `the_frame_and_the_path_tracer_agree_about_brightness_on_open_ground`,
    // which takes its ground albedo off the frame itself.
    let mirror = oracle::pathtrace::Mirror::of(oracle::pathtrace::Mirrored {
        boxes: &boxes,
        light_at: at,
        light_radius: f64::from(FLAME_RADIUS),
        colour: [1.0, 1.0, 1.0],
        intensity: 1.0,
        albedos: oracle::pathtrace::Albedos::INVENTED,
        // **The engine's own sphere, and phase 5 is what changed it from a
        // point.** A reference holding a point where the frame holds a body
        // reports the whole penumbra as a disagreement — it did, on one pixel of
        // this very scene, the hour the eight rays landed.
        body: oracle::pathtrace::ENGINE_FLAME,
        to_pixel: frame.to_pixel.as_ref(),
    });
    let seen = |brdf, seed| mirror.render(brdf, seed, TRACE_SIZE);
    let exact = seen(pt_trace::Brdf::Flat, oracle::pathtrace::FIRST_SEED);
    let verdict = oracle::pathtrace::compare(
        &exact,
        &seen(pt_trace::Brdf::Lambert, oracle::pathtrace::FIRST_SEED),
        oracle::pathtrace::Frame {
            size: TRACE_SIZE,
            drawn: &frame.drawn,
            picture: &frame.surface,
            face_rows: &frame.face_rows,
        },
    );
    eprint!("{}", verdict.report());

    // **And the shape of the soft edge, which is phase 5's own "done when".**
    // The verdict above reads both pictures as one bit a pixel, which is the
    // right question for a hard shadow and says nothing at all about a soft one:
    // a penumbra is exactly the region where that bit is arbitrary. This reads
    // the fraction instead, and measures the reference's own noise beside it so
    // that "they agree to within an eighth" is a statement with a scale under it.
    let allowed = oracle::pathtrace::PENUMBRA_ALLOWED;
    let soft = oracle::pathtrace::penumbra(
        &exact,
        &seen(pt_trace::Brdf::Flat, oracle::pathtrace::SECOND_SEED),
        oracle::pathtrace::Frame {
            size: TRACE_SIZE,
            drawn: &frame.drawn,
            picture: &frame.surface,
            face_rows: &frame.face_rows,
        },
        allowed,
    );
    eprint!("{}", soft.report(allowed));

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

    // The soft gate's own non-triviality: a penumbra of a handful of pixels is a
    // hard shadow with a rounding error on it, and would pass this for any ray
    // count at all — including one.
    assert!(
        soft.compared > 1_000,
        "only {} pixels of this frame are partly lit on both sides: {}",
        soft.compared,
        soft.report(allowed),
    );
    // **The two gates are aggregates, and that is a finding rather than a
    // convenience.** At sixty-four random samples a pixel the reference's own
    // per-pixel error is a *third of a flame* at the middle of a soft edge — it
    // disagrees with itself by that much, measured — so a per-pixel comparison
    // there is a gate on the ruler and not on the thing being measured. What is
    // sharp at this sample count is what averages: thousands of pixels turn the
    // reference's noise into a thousandth and leave a model difference standing.
    // The per-pixel count is in the report for a person to read; it is not what
    // this asserts, and `soft.over` being non-zero is the reference's variance.
    // See the backlog: a reference that stratified its own disc would make the
    // per-pixel claim available, at no extra samples.
    //
    // The mean is the triangle inequality with both terms named: the engine's own
    // estimator is worth half a ray on average, and the reference's distance from
    // the truth is its measured disagreement with *itself* over root two — two
    // independent errors either side of the same answer are root two apart.
    // Neither term is chosen to fit; both were checked by breaking them, below.
    let budget = ENGINE_MEAN + soft.noise_mean / std::f64::consts::SQRT_2;
    assert!(
        soft.mean <= budget,
        "the frame's penumbra is {:.4} of a flame from the reference's on average, past the {budget:.4} \
         that half a ray of eight plus the reference's own measured {:.4} allows\n{}",
        soft.mean,
        soft.noise_mean / std::f64::consts::SQRT_2,
        soft.report(allowed),
    );
    // **And the sharp one.** Everything above is a bound on noise, and noise is
    // symmetric: it cancels in a signed mean over thousands of pixels, where a
    // wrong *model* does not. A flame of the wrong radius, a disc sampled off
    // centre, a penumbra sitting to one side of where it belongs — each of those
    // moves this and is invisible in the numbers above.
    assert!(
        soft.bias.abs() < PENUMBRA_BIAS,
        "the frame's penumbra is {:+.4} of a flame brighter than the reference's on average, past the \
         {PENUMBRA_BIAS} a sampling difference explains — that is a model difference, not a noise \
         one\n{}",
        soft.bias,
        soft.report(allowed),
    );
}

/// The two shadow masks and their difference, as one three-strip picture —
/// `examples/boxes.rs`'s own dump, available from the gate.
///
/// **Written only where `OPENSHARD_TRACED_DUMP` names a directory**, and the
/// gate does not read it back: a test that asserted about a file would be a test
/// of the file. What this is for is the instrument `docs/lighting_rebuild.md`
/// insists on — a picture beside the path tracer's, looked at by a person — and
/// until now that picture was only reachable by running the tool on the tool's
/// own scene.
///
/// The four strips are [`oracle::pathtrace::Verdict::strips`]'s, and what each
/// of them means is stated there — including the fourth, which says *why* a grey
/// pixel of the first two is grey. This function is the file and the env var
/// around it and nothing else: the tool has the same dump, and a second copy of
/// what the colours mean is how the gate ends up drawing a rule the tool no
/// longer draws.
fn dump_masks(verdict: &oracle::pathtrace::Verdict, name: &str) {
    let Some(dir) = std::env::var_os("OPENSHARD_TRACED_DUMP") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&dir).expect("the dump directory");
    let strips = verdict.strips();
    let strips: Vec<&[u8]> = strips.iter().map(Vec::as_slice).collect();
    let path = dir.join(format!("{}_pathtrace.png", name.replace(' ', "_")));
    openshard_client_render::png::write_strips(&path, SIDE, SIDE, &strips)
        .expect("writing the mask comparison");
    eprintln!("wrote {}", path.display());
}

/// **The same gate, on three flights standing side by side** —
/// [`stair_scene`], and `docs/occluders.md`'s own shape: nine primitives, each
/// sharing a whole face with a neighbour of a *different static* at the same
/// height.
///
/// **What this can and cannot settle, because the distinction is the whole
/// value of the answer.** Both renderers are handed the same nine boxes. So a
/// disagreement here is the *walk* being wrong about a model both sides hold;
/// agreement says the walk is right about it and says **nothing** about whether
/// nine boxes are the model we want. A landing that is continuous to a person
/// and nine solids to us is a question no reference renderer can be asked — it
/// answers what the geometry does, not what the geometry should be.
///
/// The flame sits just clear of the top landing and off its east end, which is
/// the arrangement the seam question lives in: rays run *along* a surface that is
/// continuous in the world and cut into three primitives here. Just clear and
/// not in the plane — a flame buried in the landing is genuinely inside the
/// neighbouring boxes for half its own sphere, and that darkness is honest on
/// both sides, so a fixture that put it there would be asking about nothing.
#[test]
fn the_frame_and_the_path_tracer_agree_about_a_run_of_flights() {
    let Some((device, queue)) = gpu() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let boxes = stair_scene();
    // Off the east end of the run and a unit and a half over the top landing:
    // `examples/synthetic_stair`'s `OPENSHARD_LIGHT_AT=3.5,0.167 LIGHT_Z=6.5`,
    // picked there by looking at the rendered frame.
    let at = WorldSpot {
        x: 103.5,
        y: 100.167,
        z: 6.5,
    };
    let radius = 8.0_f32;
    let frame = render(
        &device,
        &queue,
        Shot {
            boxes: &boxes,
            flame: at,
            radius,
            intensity: 1.0,
            ambient: NIGHT,
            view: View::Shadow,
            // Two notches and not three: the run is three tiles across and a
            // 512-pixel canvas at 4:1 holds about that, so the top zoom would
            // put the far flight's own seam off the edge of the picture — and
            // the seam is the reason this scene exists.
            zoom: 2,
            // **A point, and the reason is in `Shot::flame_radius`.** This gate
            // asks whether the walk is right about nine abutting primitives; at
            // the shipped radius it also asks whether eight rays estimate a soft
            // edge the way sixty-four paths do, and reports the difference as a
            // disagreement — eight pixels of it, measured, all at a graze six
            // thousandths of a tile deep. The soft edge has its own gate on the
            // scene above.
            flame_radius: 0.0,
        },
    );

    let mirror = oracle::pathtrace::Mirror::of(oracle::pathtrace::Mirrored {
        boxes: &boxes,
        light_at: at,
        light_radius: f64::from(radius),
        colour: [1.0, 1.0, 1.0],
        intensity: 1.0,
        albedos: oracle::pathtrace::Albedos::INVENTED,
        // **A point on this side too, because the frame's flame is one.**
        // `ENGINE_FLAME` is the shipped sphere and it is right for every scene
        // that draws one; here the `Shot` above asked for a radius of zero, and a
        // reference holding a body where the frame holds a point reports the
        // whole penumbra as a disagreement. It did: forty-seven pixels, the hour
        // the radius became a field and this line had not followed.
        body: oracle::pathtrace::Body::Point,
        to_pixel: frame.to_pixel.as_ref(),
    });
    let seen = |brdf, seed| mirror.render(brdf, seed, TRACE_SIZE);
    let exact = seen(pt_trace::Brdf::Flat, oracle::pathtrace::FIRST_SEED);
    assert!(
        exact.is_exact(),
        "a point emitter came back an estimate: this gate's whole claim is that neither side is one",
    );
    let verdict = oracle::pathtrace::compare(
        &exact,
        &seen(pt_trace::Brdf::Lambert, oracle::pathtrace::FIRST_SEED),
        oracle::pathtrace::Frame {
            size: TRACE_SIZE,
            drawn: &frame.drawn,
            picture: &frame.surface,
            face_rows: &frame.face_rows,
        },
    );
    eprint!("{}", verdict.report());
    dump_masks(&verdict, "run of flights");
    // `OPENSHARD_TRACED_PROBE=x,y[,radius]` — two scanlines through one pixel,
    // with the world point of each side's own surface. What a dumped picture
    // cannot say is which *face* of a body a region is, and every reading here
    // that guessed that from the shape of a region guessed wrong at least once.
    if let Some(spec) = std::env::var_os("OPENSHARD_TRACED_PROBE") {
        let spec = spec.to_string_lossy();
        let numbers: Vec<u32> = spec
            .split(',')
            .map(|n| n.trim().parse().expect("OPENSHARD_TRACED_PROBE is x,y[,radius]"))
            .collect();
        eprint!(
            "{}",
            oracle::pathtrace::probe(
                &exact,
                oracle::pathtrace::Frame {
                    size: TRACE_SIZE,
                    drawn: &frame.drawn,
                    picture: &frame.surface,
                    face_rows: &frame.face_rows,
                },
                pt_trace::ImagePixel::new(numbers[0], numbers[1]),
                numbers.get(2).copied().unwrap_or(6),
            )
        );
    }

    // The same three-sided non-triviality guard the scene above carries, and it
    // matters more here: a staircase is mostly silhouette, so a threshold that
    // let the comparison shrink to the flat tops would quietly stop asking about
    // the joins.
    assert!(
        verdict.compared > 50_000,
        "only {} of {} pixels were compared — a detector that compares nothing reads exactly like a \
         detector that found nothing",
        verdict.compared,
        SIDE * SIDE,
    );
    let lit = verdict.traced_lit.iter().flatten().filter(|lit| **lit).count();
    let dark = verdict.traced_lit.iter().flatten().filter(|lit| !**lit).count();
    assert!(
        lit > 5_000 && dark > 5_000,
        "the tracer saw {lit} lit and {dark} shadowed pixels: a scene that is all one or the other \
         agrees with anything"
    );

    assert_eq!(
        verdict.interior,
        0,
        "the path tracer and the frame disagree about {} pixels of a run of flights that no edge and \
         no surface disagreement explains\n{}",
        verdict.interior,
        verdict.report(),
    );
}

/// **The same gate on a run of wall that is one primitive** — [`wall_run_scene`]
/// of a single graphic, folded by `occlusion::merge` into one box spanning three
/// tiles, against a tracer holding the three boxes it was folded from.
///
/// **What makes this non-circular, which is the whole reason it exists.** Every
/// other scene in this file hands both renderers the same list of boxes, so a
/// disagreement is the *walk* being wrong about a model both sides hold. Here
/// the two sides hold **different lists**: the frame's walk meets one merged
/// primitive, the reference meets three. They are the same point set — the merge
/// is a componentwise union of boxes sharing whole faces — so every pixel must
/// still come out the same, and any pixel that does not is the merge having
/// invented or lost volume. That is the one claim the ten CPU gates on the merge
/// cannot make: they compare the walk against itself.
///
/// A point flame and not the shipped sphere, for
/// [`the_frame_and_the_path_tracer_agree_about_a_run_of_flights`]'s own reason:
/// at a radius the comparison also asks whether eight rays estimate a soft edge
/// the way sixty-four paths do, and reports the difference as a disagreement.
/// The soft edge has its own gate on the `line` scene.
#[test]
fn the_frame_and_the_path_tracer_agree_about_a_merged_run_of_wall() {
    let Some((device, queue)) = gpu() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let boxes = wall_run_scene(&|_| 7);
    let at = wall_run_flame();
    let radius = 8.0_f32;
    let frame = render(
        &device,
        &queue,
        Shot {
            boxes: &boxes,
            flame: at,
            radius,
            intensity: 1.0,
            ambient: NIGHT,
            view: View::Shadow,
            // Three tiles across, the same footprint as the run of flights and
            // the same argument: at 4:1 the far end of the run would be off the
            // picture, and the far end is what a wrong union moves.
            zoom: 2,
            flame_radius: 0.0,
        },
    );

    // **The precondition, asserted rather than assumed.** A scene where nothing
    // merged would pass every assertion below while asking the same question the
    // rest of this file already asks — which is exactly the state this fixture
    // was written to get out of, so it is stated in the frame's own numbers and
    // not in a second build of the grid.
    assert_eq!(
        frame.primitives, 1,
        "the run of one wall came out as {} primitives: this gate is about a merged frame and \
         nothing here merged",
        frame.primitives,
    );
    assert!(
        frame.solids.windows(2).all(|pair| pair[0] == pair[1]),
        "the three boxes name {:?} — a merged run is one primitive named from every cell it \
         spans, and `Occlusion::id_of` is the join that has to survive the merge",
        frame.solids,
    );

    let mirror = oracle::pathtrace::Mirror::of(oracle::pathtrace::Mirrored {
        boxes: &boxes,
        light_at: at,
        light_radius: f64::from(radius),
        colour: [1.0, 1.0, 1.0],
        intensity: 1.0,
        albedos: oracle::pathtrace::Albedos::INVENTED,
        body: oracle::pathtrace::Body::Point,
        to_pixel: frame.to_pixel.as_ref(),
    });
    let seen = |brdf, seed| mirror.render(brdf, seed, TRACE_SIZE);
    let exact = seen(pt_trace::Brdf::Flat, oracle::pathtrace::FIRST_SEED);
    assert!(
        exact.is_exact(),
        "a point emitter came back an estimate: this gate's whole claim is that neither side is one",
    );
    let verdict = oracle::pathtrace::compare(
        &exact,
        &seen(pt_trace::Brdf::Lambert, oracle::pathtrace::FIRST_SEED),
        oracle::pathtrace::Frame {
            size: TRACE_SIZE,
            drawn: &frame.drawn,
            picture: &frame.surface,
            face_rows: &frame.face_rows,
        },
    );
    eprint!("{}", verdict.report());
    dump_masks(&verdict, "merged run of wall");

    // The same three-sided non-triviality guard every gate here carries.
    assert!(
        verdict.compared > 50_000,
        "only {} of {} pixels were compared — a detector that compares nothing reads exactly like a \
         detector that found nothing",
        verdict.compared,
        SIDE * SIDE,
    );
    let lit = verdict.traced_lit.iter().flatten().filter(|lit| **lit).count();
    let dark = verdict.traced_lit.iter().flatten().filter(|lit| !**lit).count();
    assert!(
        lit > 5_000 && dark > 5_000,
        "the tracer saw {lit} lit and {dark} shadowed pixels: a scene that is all one or the other \
         agrees with anything"
    );

    assert_eq!(
        verdict.interior,
        0,
        "the path tracer and the frame disagree about {} pixels of a merged run of wall that no edge \
         and no surface disagreement explains — the frame's walk met one primitive where the tracer \
         met the three it was folded from\n{}",
        verdict.interior,
        verdict.report(),
    );
}

/// **A merged run draws the frame its own pieces draw, pixel for pixel** — the
/// merge's own "not one pixel moves", said about the shipped renderer.
///
/// One geometry, stated twice through the same [`render`]: three whole-tile boxes
/// of one graphic, which merge into a single primitive, against the same three as
/// three graphics, which merge into nothing. `tests/lighting.rs`'s
/// `a_merged_run_answers_every_ray_the_way_its_own_pieces_did` is this twin on
/// the CPU walk; this is the same claim about the **shader's** walk, over a whole
/// frame, and it is the half that was missing: `tests/frame.rs` sweeps the shader
/// against `light::sample`, and both read one primitive list, so it gates the
/// *port* and cannot see a list that merged wrongly.
///
/// **Both sides are ours, so this is not evidence that the merge is right** — the
/// gate above is, against a renderer that shares no arithmetic with either. What
/// this adds is reach: it compares whole pictures rather than a comparison's own
/// interior pixels, so it also covers the silhouettes, the edges and the
/// penumbra that `compare` deliberately excludes.
///
/// The shipped flame radius, and that is the sensitive part: a soft edge is a
/// grey level per pixel — eighths of a flame — where a hard shadow is one bit, so
/// a union off by a fraction of a tile shows up here as a moved grey rather than
/// as nothing at all.
#[test]
fn a_merged_run_of_wall_draws_the_same_frame_as_its_own_pieces() {
    let Some((device, queue)) = gpu() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    // The walk's own output rather than a lit picture: it is what the merge could
    // move, and it is a grey level a pixel instead of a tonemapped colour.
    let (merged, pieces) = wall_run_twin(&device, &queue, wall_run_flame(), View::Shadow);

    // **The picture has to be one a merge could have moved.** Shadow, light and
    // the soft edge between them are three different readings of the walk, and a
    // frame missing any of them would agree with a wrong union about the ones it
    // has. The penumbra count is the strictest of the three: it is where the
    // answer is a fraction rather than a bit.
    let (mut blocked, mut clear, mut soft) = (0_usize, 0_usize, 0_usize);
    for pixel in 0..(SIDE * SIDE) as usize {
        let rgb = [
            merged.surface[pixel * 4],
            merged.surface[pixel * 4 + 1],
            merged.surface[pixel * 4 + 2],
        ];
        match oracle::Shade::of(rgb) {
            oracle::Shade::Blocked => blocked += 1,
            oracle::Shade::Through(value) if value > 250 => clear += 1,
            oracle::Shade::Through(_) => soft += 1,
            oracle::Shade::Unreached => {}
        }
    }
    eprintln!(
        "merged run of wall: {} primitives against {}, {blocked} blocked, {clear} clear and {soft} \
         partly lit pixels",
        merged.primitives, pieces.primitives,
    );
    assert!(
        blocked > 5_000 && clear > 5_000,
        "the frame has {blocked} blocked and {clear} clear pixels: a picture that is all one of them \
         agrees with any geometry",
    );
    assert!(
        soft > 1_000,
        "only {soft} pixels of the frame are partly lit — the soft edge is where a union off by a \
         fraction of a tile shows, and this frame has none of it",
    );

    // And the gate: the same bytes.
    let moved = moved_pixels(&merged, &pieces);
    assert!(
        moved.is_empty(),
        "{} of {} pixels differ between a merged run and its own pieces — first at {:?}, where the \
         merged frame is {:?} and the pieces are {:?}",
        moved.len(),
        SIDE * SIDE,
        moved
            .iter()
            .take(8)
            .map(|pixel| ((*pixel as u32) % SIDE, (*pixel as u32) / SIDE))
            .collect::<Vec<_>>(),
        &merged.surface[moved[0] * 4..moved[0] * 4 + 4],
        &pieces.surface[moved[0] * 4..moved[0] * 4 + 4],
    );
}

/// **Where the merge is not free, exactly** — and it is where a lit frame has
/// already thrown the difference away.
///
/// `occlusion::merge`'s own header says the one thing a merge changes is
/// *identity*: a fragment of one piece is a point of the merged primitive, so it
/// is exempt from the volume its neighbour used to be. This is that sentence with
/// a number under it, and the number was found by the fixture above going red —
/// a flame in the run's own line, half a tile north of the drawn south faces,
/// moves **thousands** of pixels of `View::Shadow` between a merged run and its
/// pieces. Not a defect in the merge: those are the fragments whose own face is
/// turned away from the flame, where the walk's exemption is the only thing
/// deciding anything at all.
///
/// So the gate is two claims and neither of them is "nothing happens":
///
/// - every pixel that moves is a **mesh face turned away from the flame** — the
///   exemption's own reach and nothing else, which is what says the merged box
///   did not grow;
/// - the **lit** frame is the same bytes, because phase 3's cosine is nothing on
///   a face turned away and the visibility behind it cannot matter.
///
/// The second is the claim that matters to a player: `View::Shadow` is an
/// instrument, and what the client draws is the other picture.
#[test]
fn a_merged_run_is_exempt_from_itself_only_where_the_cosine_is_already_nothing() {
    let Some((device, queue)) = gpu() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let at = wall_run_grazing_flame();
    let (merged, pieces) = wall_run_twin(&device, &queue, at, View::Shadow);
    let moved = moved_pixels(&merged, &pieces);

    // A fixture that moved nothing would pass the survey below vacuously, and
    // would mean this scene no longer poses the case it is named for.
    assert!(
        !moved.is_empty(),
        "a flame in the run's own line moved no pixel between a merged run and its pieces: the \
         exemption's reach is what this measures, and this scene no longer reaches it",
    );
    let flame = [at.x, at.y, at.z];
    let mut facing = Vec::new();
    for pixel in &moved {
        let texel = &merged.drawn[*pixel];
        let at_pixel = ((*pixel as u32) % SIDE, (*pixel as u32) / SIDE);
        let Some((box_index, stance, _)) = merged.face_rows.iter().find(|(_, _, id)| *id == texel.id) else {
            facing.push(format!(
                "  [pixel {at_pixel:?}] is not a mesh face at all: kind {} stance {} row {}",
                texel.kind, texel.stance, texel.id,
            ));
            continue;
        };
        let (axis, sign) = face_normal(*stance);
        let fragment = [texel.at.0, texel.at.1, texel.at.2];
        // The normals here are axis-aligned, so the sign of `N·L` is the sign of
        // one coordinate's difference — and that holds whatever scale each axis
        // is in, which `z` units and tiles are not the same one of.
        let towards = sign * (flame[axis] - fragment[axis]);
        if towards > 0.0 {
            facing.push(format!(
                "  [pixel {at_pixel:?}] box {box_index}'s {stance:?} is turned towards the flame \
                 ({towards:+.3} along axis {axis}), and the merged frame reads {:?} where its \
                 pieces read {:?}",
                &merged.surface[pixel * 4..pixel * 4 + 3],
                &pieces.surface[pixel * 4..pixel * 4 + 3],
            ));
        }
    }
    eprintln!(
        "a flame in the run's own line moves {} of {} pixels of the shadow view, every one of them \
         a face turned away from it",
        moved.len(),
        SIDE * SIDE,
    );
    assert!(
        facing.is_empty(),
        "{} of the {} pixels the merge moved are not fragments turned away from the flame — the \
         exemption reaching a surface that faces the light is the merged box having grown, which is \
         a defect and not a cost:\n{}",
        facing.len(),
        moved.len(),
        facing.iter().take(12).cloned().collect::<Vec<_>>().join("\n"),
    );

    // And the frame a player sees, on the same scene and the same flame.
    let (merged, pieces) = wall_run_twin(&device, &queue, at, View::Lit);
    let moved = moved_pixels(&merged, &pieces);
    assert!(
        moved.is_empty(),
        "{} of {} pixels of the *lit* frame differ between a merged run and its pieces — first at \
         {:?}, {:?} against {:?}. The shadow view differs on faces turned away from the flame, and \
         the cosine is what makes that cost nothing; if it shows here, it does not.",
        moved.len(),
        SIDE * SIDE,
        moved
            .iter()
            .take(8)
            .map(|pixel| ((*pixel as u32) % SIDE, (*pixel as u32) / SIDE))
            .collect::<Vec<_>>(),
        &merged.surface[moved[0] * 4..moved[0] * 4 + 4],
        &pieces.surface[moved[0] * 4..moved[0] * 4 + 4],
    );
}

/// How much brighter than the reference the frame may be, on average, over the
/// wedge scene — in shares of a channel's full scale.
///
/// **A bound on a signed mean over a quarter of a million pixels, which is a
/// bound on the model and not on the noise.** Both sides estimate — the engine
/// with eight rays, the reference with sixty-four — and that noise is symmetric,
/// so it cancels here: the reference disagrees with *itself* by `0.0067` a pixel
/// over these very pixels, and the standard error of a mean of a quarter of a
/// million such is `1.3e-5`. So this bound is a hundred and fifty times the
/// resolution of the instrument and nowhere near the size of a real defect.
///
/// **Fault-injected rather than argued.** With the cosine restored to the flame's
/// centre — the model phase 5b replaced — the frame reads `-0.0044`, twice this
/// and two hundred times the standard error, and the gate is red. With the phase
/// in, it reads `-0.0002`. That twentyfold fall is the phase's own "the frame has
/// moved towards the reference", measured rather than asserted; both numbers are
/// in `docs/lighting_rebuild.md`.
///
/// The sign is worth keeping in mind when this next moves: the wedge makes the
/// engine **darker** than the truth, because it is rays that could not have lit
/// anything being counted as shadow.
const WEDGE_BIAS: f64 = 0.002;

/// **A flame with a body has no centre, and a landing lit by one from just above
/// its own plane is where that used to show** — `docs/lighting_rebuild.md`'s
/// phase 5b, against the reference.
///
/// The scene is [`stair_scene`] again and the flame is barely clear of the top
/// landing: half a `z`, which is under [`light::FLAME_RADIUS`] in tiles, so **half
/// the flame's sphere is below the surface it is standing on**. Rays to that half
/// are traced, and near a join between two flights they leave the fragment's own
/// primitive and enter the neighbour's — so they came back blocked and darkened a
/// surface that is flush and continuous, in a wedge widest at the join. A cosine
/// taken at the flame's centre cannot cancel them, because at the centre the
/// cosine is positive.
///
/// **Why this gate reads `View::Flames` and not `View::Shadow`.** Visibility is
/// the one term phase 5b does not touch: a flame was already a body for it, so
/// [`the_frame_and_the_path_tracer_agree_about_a_run_of_flights`] and the
/// penumbra gate beside it are invariant to this phase in *both* directions, and
/// neither could have caught the defect or can now witness the fix. What changes
/// is the cosine, the falloff and the beam, and only a picture with light in it
/// shows them. See [`oracle::pathtrace::shading`].
///
/// The albedos are **one** on the reference's side and the ambient is nothing on
/// the engine's, which makes both pictures the same quantity: the sum over the
/// flame's body of `visibility × cosine × falloff²`, times colour and intensity.
/// **Pinned deliberately, not for want of a real number any more** — since
/// `docs/lighting_rebuild.md` phase 6d a box has a measurable
/// [`oracle::pathtrace::Albedos::body`] — but this gate's subject is the
/// below-horizon wedge alone, and folding a second measured quantity in would
/// be a second thing that could move the picture on a run where only one is
/// meant to.
#[test]
fn a_flame_just_over_a_landing_does_not_wedge_it_with_its_own_below_horizon_rays() {
    let Some((device, queue)) = gpu() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let boxes = stair_scene();
    // Over the middle flight's third of the top landing, half a `z` above it.
    // `light::FLAME_RADIUS` is an eighth of a tile, which is `1.375` of a `z`, so
    // the sphere's lower half is inside the landing's own boxes — which is the
    // whole fixture. Off-centre along `x` so that the joins at `x = 101` and
    // `x = 102` are at two different distances and a wedge at either has room to
    // be seen.
    let at = WorldSpot {
        x: 101.3,
        y: 100.167,
        z: 5.5,
    };
    let radius = 8.0_f32;
    const BRIGHTNESS: f32 = 1.0;
    let frame = render(
        &device,
        &queue,
        Shot {
            boxes: &boxes,
            flame: at,
            radius,
            intensity: BRIGHTNESS,
            // Nothing, so that `View::Flames` and the reference's radiance are the
            // same number: a degenerate path trace is direct light and has no
            // ambient term to match.
            ambient: openshard_client_render::light::Ambient {
                sky: [0.0; 3],
                ground: [0.0; 3],
            },
            view: View::Flames,
            zoom: 2,
            // **The shipped sphere, and the fixture is nothing without it.** At
            // radius zero there is no below-horizon half and no wedge to measure.
            flame_radius: openshard_client_render::light::FLAME_RADIUS,
        },
    );

    let mirror = oracle::pathtrace::Mirror::of(oracle::pathtrace::Mirrored {
        boxes: &boxes,
        light_at: at,
        light_radius: f64::from(radius),
        colour: [1.0, 1.0, 1.0],
        intensity: f64::from(BRIGHTNESS) * oracle::pathtrace::LAMBERT_PI,
        // One, on both surfaces: this comparison is about the light and the
        // engine's side has thrown the art away, so a reflectance on the
        // reference's side alone would be a constant factor between two pictures
        // that are otherwise the same sum.
        albedos: oracle::pathtrace::Albedos {
            ground: [1.0; 3],
            body: [1.0; 3],
        },
        body: oracle::pathtrace::ENGINE_FLAME,
        to_pixel: frame.to_pixel.as_ref(),
    });
    let seen = |seed| mirror.render(pt_trace::Brdf::Lambert, seed, TRACE_SIZE);
    let traced = seen(oracle::pathtrace::FIRST_SEED);
    let allowed = oracle::pathtrace::PENUMBRA_ALLOWED;
    let found = oracle::pathtrace::shading(
        &traced,
        &seen(oracle::pathtrace::SECOND_SEED),
        oracle::pathtrace::Frame {
            size: TRACE_SIZE,
            drawn: &frame.drawn,
            picture: &frame.surface,
            face_rows: &frame.face_rows,
        },
        allowed,
    );
    eprint!("{}", found.report(allowed));
    // **The raw picture, where a run asks for it — for comparing two runs against
    // each other rather than against the reference.** Two of phase 5b's three
    // fault injections are claims that *nothing moves*: the below-horizon skip is
    // a cost and not a second answer, and tightening the cull changes no pixel
    // because no sample of the flame is nearer than its centre. An aggregate
    // agreeing to four decimal places is not that claim; `cmp` over two dumps is,
    // and both came back zero bytes apart. Raw `RGBA8` and not a PNG: the point is
    // that the bytes are the bytes.
    if let Some(path) = std::env::var_os("OPENSHARD_WEDGE_DUMP") {
        std::fs::write(path, &frame.surface).expect("writing the frame dump");
    }

    // The same non-triviality guard every gate in this file carries: a comparison
    // that reached a handful of pixels would pass this for any renderer.
    assert!(
        found.compared > 20_000,
        "only {} pixels were compared ({} dropped as clipping) — a detector that compares nothing \
         reads exactly like one that found nothing\n{}",
        found.compared,
        found.clamped,
        found.report(allowed),
    );
    assert!(
        found.bias.abs() < WEDGE_BIAS,
        "the frame differs from the reference by {:+.4} of full scale on average — positive is \
         brighter, negative is the wedge — past the {WEDGE_BIAS} a difference of estimators \
         explains, which makes it a model difference. The reference disagrees with itself by \
         {:.4} over the same pixels.\n{}",
        found.bias,
        found.noise_mean,
        found.report(allowed),
    );
}

// **The seam census is not a gate, and that is a measurement rather than a
// preference.** A pixel-level census over a dumped mask — `tools/mask_probe.py
// seams`, and a Rust twin of it stood here for an afternoon — counts shadow
// decisions that flip across a join of two flights, and a flip there has three
// causes a picture cannot tell apart:
//
//   1. a piece of a surface shadowing that surface, which is the defect;
//   2. a *different* surface's shadow boundary happening to cross the seam column
//      — measured, four of them on this scene, and the reference tracer draws the
//      same boundary in the same place, so the geometry really does have an edge
//      there;
//   3. the walk inventing an edge, which `interior == 0` above already rules out.
//
// The tracer cannot arbitrate 1 against 2 either: it is handed the same nine boxes,
// so it reproduces a surface shadowing itself as faithfully as a real shadow. What
// tells them apart is **which solid stopped the ray**, which only the walk can say
// — so the gate for D2 is `lighting.rs`'s
// `a_landing_cut_into_three_primitives_is_not_shadowed_by_its_own_pieces`, on
// `light::Stopper`'s own `solid`. The Python census keeps its place as an
// instrument, and it now prints a pixel of each run so that a reading can be
// followed up with `OPENSHARD_TRACED_PROBE`.

/// **A face fragment's own plane is the primitive's own number, bit for bit** —
/// the one thing `docs/occluders.md`'s S3 said it had to measure before it could
/// be written.
///
/// D2's surface exemption compares a fragment's plane against a candidate box's
/// bound: skip the candidate when its extent along the fragment's normal axis
/// *ends* at the fragment's own plane. The fragment's plane comes off the
/// interpolated position the mesh pass writes; the bound comes out of the
/// primitive buffer. Two sources, one comparison, and the plan pinned what to do
/// about that in advance — measure, and if the two are not identical, carry the
/// plane from the instance row rather than introduce a tolerance.
///
/// **They are identical, and this is the measurement.** The reason is not luck:
/// every vertex of an axis-aligned face carries the *same* coordinate on that
/// face's own axis, and interpolating a value that is equal at all three corners
/// returns it exactly under the `v0 + b·(v1−v0) + c·(v2−v0)` form every
/// rasteriser uses. So the exemption may be written as an equality, and S3 adds no
/// constant.
///
/// It stays as a gate because the claim is about the *pipeline* and not about
/// arithmetic anyone can re-derive: a projection that went perspective, a vertex
/// format that lost a bit, or a driver that interpolates as `a·v0 + b·v1 + c·v2`
/// would each break the equality while leaving every picture in this file looking
/// right — and would turn the surface exemption into a rule that fires on some
/// pixels of a seam and not others.
///
/// The stair scene rather than a synthetic quad, because it is the geometry the
/// exemption is *for*: nine boxes whose faces sit on the thirds of a tile, which
/// are the coordinates with no exact `f32` at all.
#[test]
fn a_face_fragments_own_plane_is_the_primitives_own_number() {
    let Some((device, queue)) = gpu() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let boxes = stair_scene();
    let frame = render(
        &device,
        &queue,
        Shot {
            boxes: &boxes,
            flame: flame(),
            radius: FLAME_RADIUS,
            intensity: 1.0,
            ambient: NIGHT,
            view: View::Shadow,
            zoom: 2,
            flame_radius: 0.0,
        },
    );

    let mut checked = 0_usize;
    let mut off = Vec::new();
    for texel in &frame.drawn {
        if texel.kind != openshard_client_render::place::Kind::Static as u32
            || texel.stance != Stance::MeshFace as u32
        {
            continue;
        }
        let Some((box_index, stance, _)) = frame.face_rows.iter().find(|(_, _, id)| *id == texel.id) else {
            continue;
        };
        let b = &boxes[*box_index];
        // The face's own axis, and which end of the box it is the face of — the
        // vertices of that face all carry this one coordinate, so the fragment's
        // must be it as well.
        let (axis, plane) = match stance {
            Stance::FaceNorth => (1, b.min.1),
            Stance::FaceSouth => (1, b.max.1),
            Stance::FaceWest => (0, b.min.0),
            Stance::FaceEast => (0, b.max.0),
            // A lid, which is the fragment's `z`. `Upright` and the corners do not
            // occur here: `box_mesh` gives every face an axis-aligned normal.
            Stance::Flat => (2, b.max.2),
            other => panic!("a box's face came back as {other:?}"),
        };
        let at = [texel.at.0, texel.at.1, texel.at.2];
        checked += 1;
        // `plane` is `f64` off the `BoxSpec`; the vertex carried it through `as
        // f32` and the position plane is `f32`, so the number to match is the
        // `f32` one. That cast is the wire's own rounding and not a tolerance —
        // `Solid::wire_box` is the same step on the occluder side.
        if at[axis] != f64::from(plane as f32) {
            off.push(format!(
                "box {box_index}'s {stance:?}: the fragment is at {:.9} on axis {axis}, the face is \
                 at {:.9} — {:e} apart",
                at[axis],
                f64::from(plane as f32),
                at[axis] - f64::from(plane as f32),
            ));
        }
    }

    // A detector that examined nothing reads exactly like one that found nothing,
    // and every face of this scene is thousands of pixels.
    assert!(
        checked > 20_000,
        "only {checked} face fragments were examined out of {} pixels",
        SIDE * SIDE,
    );
    assert!(
        off.is_empty(),
        "{} of {checked} face fragments do not sit on their own face's plane, so the surface \
         exemption cannot be an equality and has to carry the plane from the instance row \
         instead:\n{}",
        off.len(),
        off.iter().take(12).cloned().collect::<Vec<_>>().join("\n"),
    );
    eprintln!("{checked} face fragments, every one exactly on its own face's plane");
}

/// How far the engine's penumbra may sit from the reference's *on average*, as a
/// fraction of the whole flame.
///
/// Noise cancels in a signed mean and a model difference does not, so this is a
/// far tighter number than the per-pixel budget beside it: over the few thousand
/// pixels of this scene's soft edge, the standard error of a signed mean of
/// eighth-scale noise is about a thousandth. A fortieth is thirty times that and
/// still an order of magnitude under anything a wrong radius or an off-centre
/// disc would produce.
const PENUMBRA_BIAS: f64 = 0.025;

/// What the engine's own eight-ray estimator is worth on average, as a fraction
/// of the whole flame: **half of one ray**.
///
/// Eight stratified samples of how much of a disc a fragment can see land within
/// one ray of the true share — that is what stratification buys and it is
/// `PENUMBRA_ALLOWED`'s own argument — and a quantity bounded by one ray averages
/// half of one. It is a bound and not a fit: the measured figure on this scene is
/// under half of it, with the reference's own share taken out.
const ENGINE_MEAN: f64 = 0.5 / openshard_client_render::light::SHADOW_RAYS as f64;

/// How far apart the two pictures may be, in steps of an eight-bit channel.
///
/// **A quantisation, not a tolerance.** The two sides compute the same product
/// of the same numbers in different precisions — the shader in `f32` through a
/// rasteriser, the tracer in `f64` through an analytic intersection — and then
/// round to a byte. One step is two answers landing either side of a rounding
/// boundary; anything above it is a difference in what was computed.
///
/// Two rather than one because the *fragment* differs as well as the arithmetic:
/// the shader lights the point the rasteriser wrote at the pixel's centre and
/// the tracer lights the point its own ray meets, and on a plane seen at 4:1
/// those are the same place to within a fraction of a tile — but a fraction of a
/// tile is a real distance to a falloff curve.
const QUANTISATION: u8 = 2;

/// The engine's shaded frame and the path tracer's agree about *brightness*, on
/// the one scene where nothing else can explain a difference.
///
/// **`docs/lighting_rebuild.md`'s phase 0 "done when", as a gate.** The scene is
/// the one that phase names: one flame, flat ground, no occluders. What that
/// buys is that everything the two renderers could disagree about *except* the
/// light is gone — no silhouette, so no rasteriser fill rule; no box, so no
/// invented albedo; no occluder, so no shadow ray. What is left is the falloff
/// curve, the intensity and the colour pipeline, which is exactly the
/// calibration every later phase rests on.
///
/// The three things that had to become true for it to be possible to write,
/// each of which was a difference of its own until phase 0:
///
/// - **the albedo is the same on both sides** — read off the world texture the
///   ground pass drew, not written down twice;
/// - **the flame is the same flame** — `Light`'s own colour and intensity, where
///   the reference used to carry a `6.0` picked to make its own picture
///   readable;
/// - **one curve** — both encoded by `tonemap::encode`, the second half of what
///   `blit.wesl` ends a lit frame with.
///
/// And the ambient is *nothing*, deliberately: a degenerate path trace is direct
/// light and has no ambient term, so `NIGHT` here would be a constant on one
/// side of the comparison only — and not one that could be subtracted back out
/// afterwards, since the sum passes through a tonemap.
#[test]
fn the_frame_and_the_path_tracer_agree_about_brightness_on_open_ground() {
    let Some((device, queue)) = gpu() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    // Over the middle of the anchor tile, so the pool's brightest point is in the
    // frame and its rim is too: a comparison that saw only the tail of the curve
    // would agree about a curve of nearly any shape.
    //
    // A tile up, and it used to be a quarter of one. **Phase 3 is what moved
    // it**, and not by a margin: the shading term is a cosine now, and a source
    // three `z` above flat ground is nearly *in* that ground — its cosine is
    // under a tenth over most of a 512-pixel canvas, so the picture had 812
    // bright pixels against the ten thousand this gate needs to be measuring a
    // curve at all. The height is what puts a bright core back under the flame;
    // the curve either side of it is unchanged and is still what is being judged.
    let at = WorldSpot {
        x: 100.5,
        y: 100.5,
        z: f64::from(openshard_client_render::light::Z_PER_TILE),
    };
    let dark = openshard_client_render::light::Ambient {
        sky: [0.0; 3],
        ground: [0.0; 3],
    };
    // Brighter than a torch on purpose, and not to make the picture pretty: the
    // guard below needs ten thousand pixels either side of the frame's own
    // midpoint, and with a cosine in the term the bright half is a disc under the
    // flame. At `1.0` it was four thousand — a comparison of the curve's tail
    // alone, which is a curve nearly any other curve also fits.
    const BRIGHTNESS: f32 = 3.0;
    let frame = render(
        &device,
        &queue,
        Shot {
            boxes: &[],
            flame: at,
            radius: FLAME_RADIUS,
            intensity: BRIGHTNESS,
            ambient: dark,
            view: View::Lit,
            zoom: 0,
            // The brightness gate has no occluder in it at all, so the flame's
            // own size reaches nothing: the shipped one, because a scene that is
            // about the falloff curve should be lit by the flame the client has.
            flame_radius: openshard_client_render::light::FLAME_RADIUS,
        },
    );

    // What the ground reflects, off the picture itself — the same decode the
    // tool uses, so the gate and the pictures a person looks at are about one
    // albedo rather than two.
    let land = openshard_client_render::place::Kind::Land as u32;
    let albedo = oracle::ground_albedo(&frame.drawn, &frame.world);

    let mirror = oracle::pathtrace::Mirror::of(oracle::pathtrace::Mirrored {
        boxes: &[],
        light_at: at,
        light_radius: f64::from(FLAME_RADIUS),
        colour: [1.0, 1.0, 1.0],
        // The engine's Lambert has no `1/π` and the reference's does — see
        // `LAMBERT_PI`. This is the whole of the conversion between the two
        // conventions, and it is stated once, here, rather than by either side
        // being rewritten to look like the other.
        intensity: f64::from(BRIGHTNESS) * oracle::pathtrace::LAMBERT_PI,
        // A point: this scene has no occluder, so the flame's own size cannot
        // cast anything and an exact render is the stronger reference.
        body: oracle::pathtrace::Body::Point,
        albedos: oracle::pathtrace::Albedos {
            ground: albedo,
            // No body in the scene to have one. Left at the invented value
            // rather than at zero so that a box arriving here later fails loudly
            // as a difference rather than quietly as a black shape.
            ..oracle::pathtrace::Albedos::INVENTED
        },
        to_pixel: frame.to_pixel.as_ref(),
    });
    // **`Lambert`, and phase 3 is what changed it from `Flat`.** `Brdf::Flat` is
    // a description of the engine *before* this phase — no cosine and no notion
    // of a normal anywhere in it — so leaving the gate there would have made the
    // reference agree with the renderer we have just replaced.
    let traced = mirror.render(pt_trace::Brdf::Lambert, oracle::pathtrace::FIRST_SEED, TRACE_SIZE);

    // Compared where the tracer sees the ground and the frame drew it: the two
    // agreeing what surface is there is the same precondition the shadow gate
    // has, and on this scene it is the whole of the ground plane in view.
    let (mut compared, mut worst, mut over) = (0usize, 0u8, 0usize);
    let mut worst_at = (0u32, 0u32);
    // Of the compared pixels, how many are in the bright half of the pool and
    // how many in the dark one — counted here rather than over the whole canvas,
    // because a picture is only evidence about a curve where the curve was
    // actually asked.
    let (mut bright, mut dim) = (0usize, 0usize);
    for pixel in 0..(SIDE * SIDE) as usize {
        if frame.drawn[pixel].kind != land {
            continue;
        }
        let Some(seen) = traced.pixels[pixel].seen else {
            continue;
        };
        if seen.surface != openshard_client_pathtrace::scene::Surface::Ground {
            continue;
        }
        compared += 1;
        let engine = [
            frame.surface[pixel * 4],
            frame.surface[pixel * 4 + 1],
            frame.surface[pixel * 4 + 2],
        ];
        let reference = openshard_client_render::tonemap::encode_u8(
            traced.pixels[pixel].radiance.map(|channel| channel as f32),
        );
        let apart = (0..3)
            .map(|channel| engine[channel].abs_diff(reference[channel]))
            .max()
            .expect("three channels");
        if apart > worst {
            worst = apart;
            worst_at = ((pixel as u32) % SIDE, (pixel as u32) / SIDE);
        }
        over += usize::from(apart > QUANTISATION);
        match engine[0] > 128 {
            true => bright += 1,
            false => dim += 1,
        }
    }

    eprintln!(
        "shaded frame vs path tracer, flat ground: {compared} pixels compared ({bright} bright, \
         {dim} dim), worst channel {worst} steps of 255 at {worst_at:?}, {over} past the \
         {QUANTISATION}-step quantisation"
    );

    // The scene has to be one where the answer could have been wrong: a frame
    // that drew no ground, or a flame that reached none of it, would agree
    // perfectly about nothing. The pool's own rim is inside the canvas, so both
    // a bright band and a dark one have to be there.
    assert!(
        compared > 200_000,
        "only {compared} of {} pixels were compared",
        SIDE * SIDE
    );
    assert!(
        bright > 10_000 && dim > 10_000,
        "the frame has {bright} bright and {dim} dim ground pixels: a picture that is all one \
         brightness agrees with any falloff curve at all"
    );

    assert_eq!(
        over, 0,
        "the path tracer and the shaded frame disagree about brightness on {over} of {compared} \
         ground pixels by more than {QUANTISATION} steps of 255 — worst {worst} at {worst_at:?}. \
         The scene has one flame, flat ground and no occluders, so nothing but the falloff curve, \
         the flame's own intensity and the colour pipeline can be what differs.\n\
         `cargo run --release -p openshard-client-render --example boxes` with \
         `OPENSHARD_BOXES_SCENE=flat` draws the two pictures side by side."
    );
}
