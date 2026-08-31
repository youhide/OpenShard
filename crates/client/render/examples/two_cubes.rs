//! Two literal unit cubes, nothing else: no client files, no map, no art —
//! just two [`occlusion::Solid`]s built by hand and drawn through
//! [`solids::SolidsRenderer`], the same pass the live F5 overlay and
//! `isolated_scene`'s `OPENSHARD_SCENE_SOLIDS` mode use.
//!
//! Built to answer one question directly: does a nearer solid's face
//! genuinely hide a farther solid's face behind it, or does the farther one
//! show through? `solids.rs`'s own `Style` doc already says the answer is
//! "it depends" — `opaque: false` (the live overlay's own default) blends on
//! purpose, `opaque: true` is the painter's-algorithm overwrite. What decides
//! either one is *draw order alone*: `SolidsRenderer::render`'s pipeline has
//! `depth_stencil: None`, no hardware depth test at all, so correctness rests
//! entirely on the caller handing solids to it back-to-front. This tool draws
//! the same two cubes three ways — the order `solid::standing` itself would
//! produce, and that order reversed by hand — so a wrong sort is visible by
//! comparison rather than by trusting the sort is right.
//!
//! - `OPENSHARD_CUBE_OFFSET=dx,dy` — where the second cube stands, relative to
//!   the first. Default `1,1`: south-east, the direction this camera's own
//!   "toward it" faces (`East`/`South`, `solid.rs`'s own doc) point along, so
//!   the second cube is nearer the camera and should hide part of the first.
//! - `OPENSHARD_CUBE_HEIGHT=n` — each cube's height in `z` units. Default
//!   `11` (`light::Z_PER_TILE`), so a cube whose footprint is one tile is
//!   also one tile tall on screen rather than a flat slab or a tower.
//! - `OPENSHARD_SCENE_ZOOM=n` — notches of `Zoom::scale_up`. Default `3`.
//! - `OPENSHARD_CUBE_EDGES=0` — drop `Style::edges`, so what's left is the
//!   fills alone, unmixed with the silhouette/star stroke (`Style`'s own doc:
//!   this is what shows a face wrongly surviving behind one that should have
//!   hidden it). Default `1`, matching the live overlay's own default.
//! - `OPENSHARD_FRAME_DUMP=/tmp/x` — base path; this tool writes
//!   `<path>_forward.png`, `<path>_forward_opaque.png` and
//!   `<path>_reversed.png` beside it.
//!
//! # The same two boxes, through the real lit pipeline
//!
//! Everything above runs [`solids::SolidsRenderer`], a diagnostic overlay with
//! no depth test and no lighting at all (its own module doc) — it answers
//! "does draw order hide the far box" and nothing about whether one box casts
//! a shadow on the other. This tool also builds each box's own top/`+x`/`+y`
//! faces by hand from `solid_a.space`/`solid_b.space` — the exact corners
//! `solid::Solid::faces` itself uses, unprojected, so all three faces share
//! bit-identical vertices at the box's own edges. An earlier version of this
//! tool built the box from two [`facing::Prism`](openshard_client_render::facing::Prism)s
//! instead (`Prism` only ever carries one climb axis, so a top plus a single
//! riser is all one gives) — public API only, no [`mesh::Mesh::push`] needed
//! — but each prism's own `WIDTH_OVERLAP` widens a different axis, so the two
//! risers' corners did not quite meet at the box's shared vertical edge: a
//! wedge-shaped crack, cutting into the shadow right where the two faces
//! should have met flush. `mesh::Mesh::push` is `pub` now (was `pub(crate)`)
//! so this could be built exactly instead of worked around. Both boxes run
//! through `GroundRenderer`/`MeshFaceRenderer`/`Blit`, the same three real passes
//! `examples/synthetic_stair.rs` uses for one climbable static, with depth
//! values from `depth::Order`, exactly as a real static's would be — so the
//! near box's mesh genuinely occludes the far one's rather than relying on
//! draw order the way the overlay above does. **A floor is drawn too**, so a
//! shadow has something to fall on beside the boxes' own faces: one flat,
//! hand-built land tile (an `openshard_uofiles::image::Image` filled with a
//! single colour, packed by `LandAtlas::pack` the same way
//! `LandAtlas::build` packs real art) repeated over a synthetic
//! `WorldMap::from_blocks` covering the two boxes' own bounds, and drawn by the
//! same `ground::collect`/`GroundRenderer` the real map uses. Two extra
//! frames come out of this half: `<path>_lit.png` (`View::Lit`) and
//! `<path>_shadow.png` (`View::Shadow`), both marked with the same lime
//! crosshair `synthetic_stair.rs` uses at the flame's own projected
//! position.
//!
//! - `OPENSHARD_LIGHT_AT=dx,dy` — the flame's position, offset from cube A's
//!   own tile. Default `2.5,2.5`: past cube B on the diagonal the two cubes
//!   already stand on, so B sits between the flame and A's East face and
//!   should shadow it.
//! - `OPENSHARD_LIGHT_Z` / `OPENSHARD_LIGHT_RADIUS` — default `6` and `8`,
//!   reaching over both cubes' own height (`OPENSHARD_CUBE_HEIGHT`'s default
//!   `11`) rather than being lost under it.
//!
//! ```sh
//! OPENSHARD_FRAME_DUMP=/tmp/two_cubes \
//!     cargo run --release -p openshard-client-render --example two_cubes
//! ```

use std::path::PathBuf;

use openshard_client_render::camera::{
    Camera,
    RealPixel,
    TileBounds,
    Zoom,
};
use openshard_client_render::cutaway::Cutaway;
use openshard_client_render::occlusion::{
    Builder,
    Shape,
};
use openshard_client_render::solids::{
    Frame,
    SolidsRenderer,
    Style,
};
use openshard_map::grid::BlockExtent;
use openshard_protocol::wire::Graphic;
use openshard_protocol::world::Point;
use openshard_tiles::{
    StaticTile,
    TileFlags,
};

fn env_opt(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

fn env_or(name: &str, default: &str) -> String {
    env_opt(name).unwrap_or_else(|| default.to_string())
}

fn parse_pair(spec: &str) -> (i32, i32) {
    let (a, b) = spec
        .split_once(',')
        .unwrap_or_else(|| panic!("wanted `a,b`, got {spec:?}"));
    (
        a.trim().parse().unwrap_or_else(|_| panic!("{a:?}")),
        b.trim().parse().unwrap_or_else(|_| panic!("{b:?}")),
    )
}

fn parse_pair_f32(spec: &str) -> (f32, f32) {
    let (a, b) = spec
        .split_once(',')
        .unwrap_or_else(|| panic!("wanted `a,b`, got {spec:?}"));
    (
        a.trim().parse().unwrap_or_else(|_| panic!("{a:?}")),
        b.trim().parse().unwrap_or_else(|_| panic!("{b:?}")),
    )
}

fn gpu() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).ok()?;
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        required_limits: openshard_client_render::gbuffer::required_limits(),
        ..Default::default()
    }))
    .ok()
}

/// A lime crosshair on a black backing plate, centred at `at` — copied from
/// `examples/synthetic_stair.rs`'s own marker: "is the light behind the box
/// or in front of it" is faster to answer by looking than by reading
/// `OPENSHARD_LIGHT_AT` back, and the black backing is what keeps it visible
/// crossing an already-white pixel, not just a black one.
fn mark_crosshair(pixels: &mut [u8], width: u32, height: u32, at: (i32, i32)) {
    let mut mark = |dx: i32, dy: i32, colour: [u8; 3]| {
        let (px, py) = (at.0 + dx, at.1 + dy);
        if px < 0 || py < 0 || px as u32 >= width || py as u32 >= height {
            return;
        }
        let offset = ((py as u32 * width + px as u32) * 4) as usize;
        pixels[offset] = colour[0];
        pixels[offset + 1] = colour[1];
        pixels[offset + 2] = colour[2];
        pixels[offset + 3] = 255;
    };
    for dy in -20..=20i32 {
        for dx in -20..=20i32 {
            if dx.abs() > 2 && dy.abs() > 2 {
                continue;
            }
            mark(dx, dy, [0, 0, 0]);
        }
    }
    for dy in -18..=18i32 {
        for dx in -18..=18i32 {
            if dx.abs() > 1 && dy.abs() > 1 {
                continue;
            }
            mark(dx, dy, [80, 255, 0]);
        }
    }
}

fn dump(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    surface: &wgpu::Texture,
    width: u32,
    height: u32,
    path: &PathBuf,
    mark: Option<(i32, i32)>,
) {
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label:              Some("readback"),
        size:               u64::from(width) * u64::from(height) * 4,
        usage:              wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture:   surface,
            mip_level: 0,
            origin:    wgpu::Origin3d::ZERO,
            aspect:    wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset:         0,
                bytes_per_row:  Some(width * 4),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([encoder.finish()]);
    let slice = readback.slice(..);
    slice.map_async(wgpu::MapMode::Read, |result| {
        result.expect("mapping a buffer this example just wrote")
    });
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("waiting on our own submission");
    let mut pixels = slice
        .get_mapped_range()
        .expect("the map completed above")
        .to_vec();
    if let Some(at) = mark {
        mark_crosshair(&mut pixels, width, height, at);
    }
    std::fs::write(
        path,
        openshard_client_render::png::encode_rgba(width, height, &pixels),
    )
    .expect("writing the frame");
    eprintln!("wrote {}", path.display());
}

fn main() {
    let (device, queue) = gpu().expect("an adapter");

    let (dx, dy) = parse_pair(&env_or("OPENSHARD_CUBE_OFFSET", "1,1"));
    let height: u8 = env_or("OPENSHARD_CUBE_HEIGHT", "11").parse().expect("a number");
    let edges: u8 = env_or("OPENSHARD_CUBE_EDGES", "1").parse().expect("0 or 1");
    let edges = edges != 0;

    let a = Point::new(100, 100, 0);
    let b = Point::new((100 + dx) as u16, (100 + dy) as u16, 0);

    // NO_SHOOT so it occludes light at all (a crate's own flags do not:
    // `occlusion::opacity`'s own doc). Not climbable and not FLOOR, and
    // `Shape::UNREAD` says the art named no edge — together that is the
    // `0 | Edges::ANY` arm of `Builder::add`, one whole-tile body rather than a
    // wall's single named-edge panel.
    let cube_tile = StaticTile {
        flags: TileFlags::new(TileFlags::NO_SHOOT),
        height,
        ..StaticTile::default()
    };
    let bounds = TileBounds {
        min_x: 90,
        max_x: 110,
        min_y: 90,
        max_y: 110,
    };
    let mut builder = Builder::new(bounds);
    builder.add(a.x, a.y, a.z, Graphic(1), &cube_tile, Shape::UNREAD);
    builder.add(b.x, b.y, b.z, Graphic(1), &cube_tile, Shape::UNREAD);
    let occlusion = builder.finish(&Cutaway::OPEN);

    let solid_a = *occlusion
        .solids_at(i32::from(a.x), i32::from(a.y))
        .next()
        .expect("cube A");
    let solid_b = *occlusion
        .solids_at(i32::from(b.x), i32::from(b.y))
        .next()
        .expect("cube B");
    eprintln!(
        "A: x {:.1}..{:.1} y {:.1}..{:.1} z {:.1}..{:.1}",
        solid_a.space.min.x,
        solid_a.space.max.x,
        solid_a.space.min.y,
        solid_a.space.max.y,
        solid_a.space.min.z,
        solid_a.space.max.z
    );
    eprintln!(
        "B: x {:.1}..{:.1} y {:.1}..{:.1} z {:.1}..{:.1}",
        solid_b.space.min.x,
        solid_b.space.max.x,
        solid_b.space.min.y,
        solid_b.space.max.y,
        solid_b.space.min.z,
        solid_b.space.max.z
    );

    const RED: [f32; 3] = [255.0, 40.0, 40.0];
    const BLUE: [f32; 3] = [40.0, 120.0, 255.0];

    let forward = vec![(solid_a.space, RED), (solid_b.space, BLUE)];
    let mut reversed = forward.clone();
    reversed.reverse();

    let (width, height_px): (u32, u32) = (512, 512);
    let zoom_notches: u32 = env_or("OPENSHARD_SCENE_ZOOM", "3").parse().expect("a number");
    let mut camera = Camera::new(Point::new((a.x + b.x) / 2, (a.y + b.y) / 2, 0), width, height_px);
    let mut zoom = Zoom::ONE;
    for _ in 0..zoom_notches {
        zoom = zoom.scale_up();
    }
    camera.zoom_about(RealPixel::new((width / 2) as i32, (height_px / 2) as i32), zoom);

    let format = wgpu::TextureFormat::Rgba8Unorm;
    let base = env_opt("OPENSHARD_FRAME_DUMP").unwrap_or_else(|| "two_cubes".to_string());

    for (suffix, order, style) in [
        ("_forward.png", &forward, Style { edges, opaque: false }),
        ("_forward_opaque.png", &forward, Style { edges, opaque: true }),
        ("_reversed.png", &reversed, Style { edges, opaque: false }),
        ("_reversed_opaque.png", &reversed, Style { edges, opaque: true }),
    ] {
        let surface = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("surface"),
            size: wgpu::Extent3d {
                width,
                height: height_px,
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
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("clear"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view:           &surface_view,
                depth_slice:    None,
                resolve_target: None,
                ops:            wgpu::Operations {
                    load:  wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        let mut solids_pass = SolidsRenderer::new(&device, format);
        solids_pass.render(
            &device,
            &queue,
            &mut encoder,
            Frame {
                target: &surface_view,
                size:   (width, height_px),
                rect:   openshard_client_render::blit::ViewportRect {
                    x: 0,
                    y: 0,
                    width,
                    height: height_px,
                },
            },
            &camera,
            order,
            style,
        );
        queue.submit([encoder.finish()]);
        dump(
            &device,
            &queue,
            &surface,
            width,
            height_px,
            &PathBuf::from(format!("{base}{suffix}")),
            None,
        );
    }

    // ---- The same two boxes, through the real lit pipeline: does the near
    // one's mesh cast a shadow on the far one's? See the module doc's own
    // "through the real lit pipeline" section for why this is a `Prism`
    // (top + one `+x` riser) rather than `SolidsRenderer`'s three faces.
    use openshard_client_render::atlas::{
        LandAtlas,
        TexmapAtlas,
    };
    use openshard_client_render::blit::Frame as BlitFrame;
    use openshard_client_render::camera::project_exact;
    use openshard_client_render::debug::View;
    use openshard_client_render::depth;
    use openshard_client_render::geometry::Vec2;
    use openshard_client_render::light::{
        Light,
        Lighting,
        NIGHT,
    };
    use openshard_client_render::mesh_face::{
        MeshFaceRow,
        MeshFaceVertex,
    };
    use openshard_client_render::place::Stance;
    use openshard_client_render::renderer::{
        self,
        GroundRenderer,
        MeshFaceRenderer,
        Target,
    };

    // Two independent `Prism`s (one per riser: `facing::Prism` only ever
    // carries one climb axis) each widen their own footprint by
    // `facing::WIDTH_OVERLAP`, on two different axes — the top, the East
    // riser and the South riser then don't share exact corners at the box's
    // own vertical edge, and the crack shows up as a wedge cut into the
    // shadow right where the two faces should meet flush (found by looking
    // at a rendered frame, not by reading the arithmetic). Building the three
    // faces by hand from `solid_a.space`/`solid_b.space` — the exact corners
    // `solid::Solid::faces` itself uses, unprojected — gives all three faces
    // bit-identical shared vertices instead, the same fix `facing.rs`'s own
    // `SEAM_OVERLAP`/`WIDTH_OVERLAP` exist to work around for a single
    // `Prism`'s tread/riser seam.
    fn box_mesh(solid: openshard_client_render::solid::Solid) -> openshard_client_render::mesh::Mesh {
        use openshard_client_render::camera::WorldSpot;
        use openshard_client_render::mesh::{
            Face,
            Mesh,
        };

        let (lo, hi) = (solid.min, solid.max);
        let mut mesh = Mesh::EMPTY;
        mesh.push(
            Face::new(
                &[
                    WorldSpot {
                        x: lo.x,
                        y: lo.y,
                        z: hi.z,
                    },
                    WorldSpot {
                        x: hi.x,
                        y: lo.y,
                        z: hi.z,
                    },
                    WorldSpot {
                        x: hi.x,
                        y: hi.y,
                        z: hi.z,
                    },
                    WorldSpot {
                        x: lo.x,
                        y: hi.y,
                        z: hi.z,
                    },
                ],
                [0.0, 0.0, 1.0],
            )
            .expect("a 4-corner ring"),
        );
        mesh.push(
            Face::new(
                &[
                    WorldSpot {
                        x: hi.x,
                        y: lo.y,
                        z: hi.z,
                    },
                    WorldSpot {
                        x: hi.x,
                        y: hi.y,
                        z: hi.z,
                    },
                    WorldSpot {
                        x: hi.x,
                        y: hi.y,
                        z: lo.z,
                    },
                    WorldSpot {
                        x: hi.x,
                        y: lo.y,
                        z: lo.z,
                    },
                ],
                [1.0, 0.0, 0.0],
            )
            .expect("a 4-corner ring"),
        );
        mesh.push(
            Face::new(
                &[
                    WorldSpot {
                        x: lo.x,
                        y: hi.y,
                        z: hi.z,
                    },
                    WorldSpot {
                        x: hi.x,
                        y: hi.y,
                        z: hi.z,
                    },
                    WorldSpot {
                        x: hi.x,
                        y: hi.y,
                        z: lo.z,
                    },
                    WorldSpot {
                        x: lo.x,
                        y: hi.y,
                        z: lo.z,
                    },
                ],
                [0.0, 1.0, 0.0],
            )
            .expect("a 4-corner ring"),
        );
        mesh
    }

    let base_tile = depth::base_for(i32::from((a.x + b.x) / 2), i32::from((a.y + b.y) / 2));
    let depth_of = |p: Point| {
        depth::Order {
            tile:       i32::from(p.x) + i32::from(p.y),
            priority_z: depth::static_priority_z(p.z, &cube_tile),
        }
        .to_depth(base_tile)
    };

    let mut rows: Vec<MeshFaceRow> = Vec::new();
    let mut vertices: Vec<MeshFaceVertex> = Vec::new();
    for (tile, solid) in [(a, solid_a.space), (b, solid_b.space)] {
        let mesh = box_mesh(solid);
        let d = depth_of(tile);
        // Which *solid* of the grid this cube is, off the grid built above — one
        // `Builder::add`, one solid, so `Part::ONLY` names it. Without it every
        // face of a cube would be a point of nothing and the cube would shadow
        // itself; `docs/lighting_rebuild.md` phase 4, and `id_of` is the join it
        // pays for.
        let mine = occlusion
            .id_of(
                i32::from(tile.x),
                i32::from(tile.y),
                openshard_client_render::occlusion::Owner::new(tile.z, Graphic(1)),
                openshard_client_render::occlusion::Part::ONLY,
            )
            .unwrap_or_else(|| panic!("the cube at {tile:?} is not in the grid this tool built"));
        for face in mesh.faces() {
            let id = rows.len() as u32;
            rows.push(MeshFaceRow {
                tile:   (tile.x, tile.y),
                stance: Stance::of_normal(face.normal).expect("a prism face's own axis-aligned normal"),
                solid:  mine.raw(),
            });
            for corner in face.fan() {
                let screen = camera.to_view_exact(project_exact(corner));
                vertices.push(MeshFaceVertex {
                    screen,
                    world: [corner.x as f32, corner.y as f32, corner.z as f32],
                    depth: d,
                    id,
                    tile: [f32::from(tile.x), f32::from(tile.y)],
                    normal: face.normal,
                    // This tool is about the crack between two silhouettes, not
                    // about colour — a flat mid-grey the same as `two_cubes`'
                    // own two cubes' picture is not asking anything of.
                    colour: [0.5, 0.5, 0.5],
                });
            }
        }
    }
    eprintln!("{} mesh faces, {} vertices", rows.len(), vertices.len());
    for (id, row) in rows.iter().enumerate() {
        let corners: Vec<&MeshFaceVertex> = vertices.iter().filter(|v| v.id == id as u32).collect();
        let (mut minx, mut maxx, mut miny, mut maxy) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
        for c in &corners {
            minx = minx.min(c.screen.x);
            maxx = maxx.max(c.screen.x);
            miny = miny.min(c.screen.y);
            maxy = maxy.max(c.screen.y);
        }
        eprintln!(
            "face {id}: tile {:?}, stance {:?}, screen x {minx:.1}..{maxx:.1}, y {miny:.1}..{maxy:.1}",
            row.tile, row.stance,
        );
    }

    let world = openshard_client_render::blit::world_texture(&device, width, height_px);
    let world_view = world.create_view(&wgpu::TextureViewDescriptor::default());
    let gbuffer = openshard_client_render::gbuffer::Gbuffer::new(&device, width, height_px);
    let gbuffer_views = gbuffer.views();
    let depth_tex = renderer::depth_texture(&device, width, height_px);
    let depth_view = depth_tex.create_view(&wgpu::TextureViewDescriptor::default());

    // A floor for a shadow to fall on: one flat, hand-built land tile —
    // `openshard_uofiles::image::Image` is `LandAtlas::pack`'s own input and
    // needs no client file, the same way `Shape::UNREAD` let the cubes
    // themselves skip real art. `WorldMap::from_blocks` repeats it everywhere,
    // which costs nothing (`isolated_scene.rs`'s own doc: no statics, and
    // this scene never asks it for any) and is simpler than tracking which
    // blocks the bounds above actually touch.
    const FLOOR_TILE: openshard_tiles::LandTileId = openshard_tiles::LandTileId(3);
    let floor_pixel = openshard_uofiles::color::Color16((20 << 10) | (20 << 5) | 20);
    let floor_image = openshard_uofiles::image::Image::new(
        openshard_uofiles::art::LAND_TILE_SIZE,
        openshard_uofiles::art::LAND_TILE_SIZE,
        vec![floor_pixel; usize::from(openshard_uofiles::art::LAND_TILE_SIZE).pow(2)],
    );
    let blocks = (bounds.max_x as u32).div_ceil(openshard_map::map::BLOCK_SIZE) + 1;
    let synthetic_map = openshard_map::map::WorldMap::from_blocks(
        BlockExtent {
            wide: blocks,
            down: blocks,
        },
        |_x, _y| {
            openshard_map::map::LandCell {
                tile: FLOOR_TILE,
                z:    0,
            }
        },
    );

    let land = LandAtlas::pack([(FLOOR_TILE, floor_image)]).expect("one flat tile always fits");
    let texmaps = TexmapAtlas::pack([]).expect("nothing always fits");
    let ground_quads =
        openshard_client_render::ground::collect(&synthetic_map, &camera, &land, &texmaps, &Cutaway::OPEN);
    eprintln!("{} ground quads", ground_quads.len());
    let mut ground_pass = GroundRenderer::new(&device, &queue, format, &land, &texmaps);
    let mut mesh_pass = MeshFaceRenderer::new(&device);
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    let target = Target {
        gbuffer: &gbuffer_views,
        view: &world_view,
        depth: &depth_view,
        width,
        height: height_px,
        projection: camera.projection(),
    };
    ground_pass.render(&device, &queue, &mut encoder, target, &ground_quads);
    mesh_pass.render(&device, &queue, &mut encoder, target, &vertices, &rows);
    queue.submit([encoder.finish()]);

    let (ldx, ldy) = parse_pair_f32(&env_or("OPENSHARD_LIGHT_AT", "2.5,2.5"));
    let light_z: f32 = env_or("OPENSHARD_LIGHT_Z", "6").parse().expect("a number");
    let light_radius: f32 = env_or("OPENSHARD_LIGHT_RADIUS", "8").parse().expect("a number");
    eprintln!("light: at ({ldx:+}, {ldy:+}) of cube A's tile, z {light_z}, radius {light_radius}");
    let mut lighting = Lighting {
        ambient: NIGHT,
        lights: vec![Light {
            at:        Vec2::new(f32::from(a.x) + ldx, f32::from(a.y) + ldy),
            z:         light_z,
            radius:    light_radius,
            color:     [1.0, 1.0, 1.0],
            intensity: 1.0,
            beam:      None,
        }],
        occlusion,
        sun: None,
        view: View::Lit,
        flame_radius: openshard_client_render::light::FLAME_RADIUS,
        shadow_rays: openshard_client_render::light::ShadowRays::DEFAULT,
        dead: false,
    };
    // Where the flame itself projects to, marked directly on every dumped
    // frame below: `examples/synthetic_stair.rs`'s own trick, since "is the
    // light behind the box or in front of it" is faster to answer by looking
    // than by reading `OPENSHARD_LIGHT_AT` back.
    let projection = camera.projection();
    let light_screen = camera.to_view_exact(project_exact(openshard_client_render::camera::WorldSpot {
        x: f64::from(a.x) + f64::from(ldx),
        y: f64::from(a.y) + f64::from(ldy),
        z: f64::from(light_z),
    }));
    let light_pixel = (
        (light_screen.x - projection.origin.x) * projection.scale + width as f32 * 0.5,
        (light_screen.y - projection.origin.y) * projection.scale + height_px as f32 * 0.5,
    );
    let light_mark = (light_pixel.0.round() as i32, light_pixel.1.round() as i32);
    eprintln!("light pixel: {light_mark:?}");

    let dummy_instances = openshard_client_render::blit::dummy_instances(&device);
    let mut blit = openshard_client_render::blit::Blit::new(&device, format);
    for view in [View::Lit, View::Shadow] {
        lighting.view = view;
        let surface = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("surface"),
            size: wgpu::Extent3d {
                width,
                height: height_px,
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
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        blit.render(
            &device,
            &queue,
            &mut encoder,
            BlitFrame {
                target:           &surface_view,
                world:            &world_view,
                gbuffer:          &gbuffer_views,
                face_instances:   &dummy_instances,
                item_instances:   &dummy_instances,
                mobile_instances: &dummy_instances,
                mesh_instances:   mesh_pass.rows_buffer(),
                ground_instances: ground_pass.instances_buffer(),
                zoom:             Zoom::ONE,
                rect:             openshard_client_render::blit::ViewportRect {
                    x: 0,
                    y: 0,
                    width,
                    height: height_px,
                },
            },
            &lighting,
        );
        queue.submit([encoder.finish()]);
        dump(
            &device,
            &queue,
            &surface,
            width,
            height_px,
            &PathBuf::from(format!("{base}_{}.png", view.name())),
            Some(light_mark),
        );
    }
}
