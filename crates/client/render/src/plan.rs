//! A plan view of a frame's lighting: the light itself, from above, with no art
//! and no projection under it.
//!
//! # Why a second picture exists at all
//!
//! Everything else that draws this pass draws it *through* the isometric camera,
//! over the client's own sprites. That is the picture a player sees and it is a
//! poor instrument: a pool's shape is folded through a projection that turns a
//! circle into a diamond-ish blob, half of it is behind a wall's sprite, and what
//! is left is multiplied by art nobody controls. Asking "is this flame's falloff
//! a circle" of that picture is asking three questions at once.
//!
//! So this draws the same pass — the real [`blit`](crate::blit), the real
//! shader, the real [`Lighting`] — over a *synthetic* place attachment that says
//! every pixel is flat ground on the tile straight above it, one tile to a square
//! of `scale` pixels. The world image is white, so what comes out is the
//! multiplier itself. A circle in the world is a circle here, a tile is a square,
//! and a wall is a line one can point at.
//!
//! It is not a second implementation of anything: the arithmetic is `blit.wgsl`'s
//! and the only thing this file makes up is where a pixel says it is. That is the
//! same seam `tests/frame.rs`'s parity fixture already uses, lifted out of the
//! tests so that a person looking at a bug can get a picture without writing one.
//!
//! # What it draws over the picture
//!
//! A pool that is the wrong shape and a pool that is the right shape *behind a
//! wall nobody can see* look identical. So [`Picture::mark`] strokes the reasons
//! on top: the panel of every occluding cell on the side it stands on, the tile
//! grid, each flame's position and the rim of its radius. What is left dark after
//! that has a visible cause or it is a bug.

use crate::camera::{TileBounds, Zoom};
use crate::debug::View;
use crate::facing::Face;
use crate::light::Lighting;
use crate::occlusion::{self, Cell};

/// One drawn plan, and what it was drawn of.
///
/// RGBA rows, top-left first, `width * 4` bytes each — the layout the readback
/// produces and the one [`Picture::png`] writes out.
#[derive(Clone, Debug)]
pub struct Picture {
    /// The tiles it covers, inclusive.
    pub bounds: TileBounds,
    /// How many pixels one tile is, on both axes.
    pub scale: u32,
    /// Its width in pixels: `bounds.width() * scale`.
    pub width: u32,
    /// Its height.
    pub height: u32,
    /// The pixels, RGBA8.
    pub pixels: Vec<u8>,
}

impl Picture {
    /// One pixel.
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let at = ((y * self.width + x) * 4) as usize;
        [
            self.pixels[at],
            self.pixels[at + 1],
            self.pixels[at + 2],
            self.pixels[at + 3],
        ]
    }

    /// Where a point of the world lands in this picture, in pixels.
    ///
    /// Tiles, fractional: `(100.5, 100.5)` is the middle of tile `(100, 100)`.
    /// Nothing clamps — a caller asking about a point outside the bounds gets a
    /// pixel outside the picture and is expected to notice.
    pub fn at(&self, x: f32, y: f32) -> (i32, i32) {
        (
            ((x - self.bounds.min_x as f32) * self.scale as f32) as i32,
            ((y - self.bounds.min_y as f32) * self.scale as f32) as i32,
        )
    }

    /// How bright the middle of one tile came out, `0..=1`, averaged over the
    /// three channels.
    ///
    /// The middle and not a corner: a tile's own square is `scale` pixels across
    /// and the fraction the shader was given runs across it, so an edge pixel is
    /// half a statement about the neighbour.
    pub fn tile(&self, x: u16, y: u16) -> f32 {
        let (px, py) = self.at(f32::from(x) + 0.5, f32::from(y) + 0.5);
        let pixel = self.pixel(
            px.clamp(0, self.width as i32 - 1) as u32,
            py.clamp(0, self.height as i32 - 1) as u32,
        );
        pixel[..3].iter().map(|c| f32::from(*c) / 255.0).sum::<f32>() / 3.0
    }

    /// The picture as a PNG, which is what a person opens.
    pub fn png(&self) -> Vec<u8> {
        crate::png::encode_rgba(self.width, self.height, &self.pixels)
    }

    /// Paint one pixel, if it is inside the picture.
    fn plot(&mut self, x: i32, y: i32, color: [u8; 3]) {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return;
        }
        let at = ((y as u32 * self.width + x as u32) * 4) as usize;
        self.pixels[at..at + 3].copy_from_slice(&color);
    }

    /// A straight run of pixels, one axis at a time — every line this file draws
    /// is a tile boundary or a marker, and both are axis-aligned.
    fn line(&mut self, from: (i32, i32), to: (i32, i32), color: [u8; 3]) {
        if from.0 == to.0 {
            for y in from.1.min(to.1)..=from.1.max(to.1) {
                self.plot(from.0, y, color);
            }
        } else {
            for x in from.0.min(to.0)..=from.0.max(to.0) {
                self.plot(x, from.1, color);
            }
        }
    }

    /// Every dashed second pixel of a run: a boundary that is *stated* rather than
    /// solid — a tile grid, or a radius nothing physically stops.
    fn dashed(&mut self, from: (i32, i32), to: (i32, i32), color: [u8; 3]) {
        if from.0 == to.0 {
            for y in (from.1.min(to.1)..=from.1.max(to.1)).step_by(4) {
                self.plot(from.0, y, color);
                self.plot(from.0, y + 1, color);
            }
        } else {
            for x in (from.0.min(to.0)..=from.0.max(to.0)).step_by(4) {
                self.plot(x, from.1, color);
                self.plot(x + 1, from.1, color);
            }
        }
    }

    /// Mark the tile seams of an [`elevation`]: one dashed line down the picture
    /// wherever one tile of the run ends and the next begins.
    ///
    /// The seams are the whole subject of an elevation. A run of wall is drawn by
    /// as many sprites as it has tiles and lit as one surface, and every defect
    /// this picture is for shows up *at* a seam — so a person reading it needs to
    /// know where they are without counting pixels.
    pub fn mark_seams(&mut self) {
        let scale = self.scale as i32;
        for seam in 0..=(self.width as i32 / scale) {
            self.dashed((seam * scale, 0), (seam * scale, self.height as i32 - 1), GRID);
        }
    }

    /// Draw the reasons over the picture: the tile grid, what occludes and on
    /// which side of its tile, and where each flame is with the rim of its reach.
    ///
    /// This is the half of the instrument that says *why*. A dark wedge in a pool
    /// is a bug or a wall, and the two look the same until the wall is drawn on
    /// top of it — which is the same argument `docs/lighting.md`'s step 14 makes
    /// for the occluder boxes in the client, arriving where the picture is a plan
    /// and a panel is a line rather than a projected box.
    pub fn mark(&mut self, lighting: &Lighting) {
        let scale = self.scale as i32;
        // The grid first, so everything else is drawn over it.
        for x in self.bounds.min_x..=self.bounds.max_x + 1 {
            let px = (x - self.bounds.min_x) * scale;
            self.dashed((px, 0), (px, self.height as i32 - 1), GRID);
        }
        for y in self.bounds.min_y..=self.bounds.max_y + 1 {
            let py = (y - self.bounds.min_y) * scale;
            self.dashed((0, py), (self.width as i32 - 1, py), GRID);
        }

        for (x, y, cell) in lighting.occlusion.boxes() {
            let (left, top) = ((x - self.bounds.min_x) * scale, (y - self.bounds.min_y) * scale);
            let (right, bottom) = (left + scale - 1, top + scale - 1);
            let color = panel_color(&cell);
            if cell.edges == 0 {
                // A lid — a floor or a roof. It has no side to draw, so it is the
                // whole square, dashed: the ray is stopped by its `z` span alone.
                self.dashed((left, top), (right, top), color);
                self.dashed((left, bottom), (right, bottom), color);
                self.dashed((left, top), (left, bottom), color);
                self.dashed((right, top), (right, bottom), color);
                continue;
            }
            // A panel stands on a side of its tile, and that side is the line the
            // ray has to cross. Two pixels thick, so a wall reads as a wall at a
            // glance rather than as a hairline.
            for (edge, from, to) in [
                (occlusion::EDGE_NORTH, (left, top), (right, top)),
                (occlusion::EDGE_SOUTH, (left, bottom), (right, bottom)),
                (occlusion::EDGE_WEST, (left, top), (left, bottom)),
                (occlusion::EDGE_EAST, (right, top), (right, bottom)),
            ] {
                if cell.edges & edge == 0 {
                    continue;
                }
                self.line(from, to, color);
                let inward = match edge {
                    occlusion::EDGE_NORTH => (0, 1),
                    occlusion::EDGE_SOUTH => (0, -1),
                    occlusion::EDGE_WEST => (1, 0),
                    _ => (-1, 0),
                };
                self.line(
                    (from.0 + inward.0, from.1 + inward.1),
                    (to.0 + inward.0, to.1 + inward.1),
                    color,
                );
            }
        }

        for light in &lighting.lights {
            let (px, py) = self.at(light.at.x, light.at.y);
            // A cross at the flame, and the rim of its radius dashed around it:
            // outside that circle the shader does not enter the walk at all, so a
            // pool that stops short of it stopped for a reason and one that runs
            // past it is impossible.
            self.line((px - 4, py), (px + 4, py), FLAME);
            self.line((px, py - 4), (px, py + 4), FLAME);
            let radius = light.radius * self.scale as f32;
            // Dashed, four degrees on and four off: a rim that is a bound rather
            // than a thing, and a solid ring would read as an edge in the light.
            for step in (0..360).filter(|step| step % 8 < 4) {
                let angle = (step as f32).to_radians();
                self.plot(
                    px + (radius * angle.cos()) as i32,
                    py + (radius * angle.sin()) as i32,
                    FLAME_RIM,
                );
            }
        }
    }
}

/// The tile grid: dark, because it is under everything and is only there to
/// count with.
const GRID: [u8; 3] = [40, 40, 55];

/// A flame's own position.
const FLAME: [u8; 3] = [255, 40, 40];

/// The rim of its reach: the same red, dimmed, because it is a bound rather than
/// a thing.
const FLAME_RIM: [u8; 3] = [140, 30, 30];

/// What colour a cell's panel is drawn: from bone for a wall to glass for a pane,
/// the same scale the client's occluder boxes use so that a person reading both
/// pictures reads one legend.
fn panel_color(cell: &Cell) -> [u8; 3] {
    let solid = f32::from(cell.opacity) / 255.0;
    [
        (90.0 + 130.0 * solid) as u8,
        (200.0 - 40.0 * solid) as u8,
        (230.0 - 60.0 * solid) as u8,
    ]
}

/// Draw one frame's lighting from above.
///
/// `bounds` is what to cover and `scale` how many pixels a tile gets. The
/// device and the queue are the caller's — this crate never makes an adapter, and
/// the callers that have one are the tests and the app.
///
/// The `view` is [`View`]'s, unchanged: [`View::Lit`] over a white world is the
/// multiplier itself, and every other value is the diagnostic the shader already
/// draws. Which is the point of doing this through the real blit — a plan view
/// with its own arithmetic would be a second implementation to keep in step.
pub fn draw(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    lighting: &Lighting,
    view: View,
    bounds: TileBounds,
    scale: u32,
) -> Picture {
    let width = bounds.width().max(1) as u32 * scale;
    let height = bounds.height().max(1) as u32 * scale;
    // Every pixel is flat ground on the tile above it, with the fraction of the
    // tile it sits at. `crate::place`'s packing, written here rather than built
    // through `Place` because there is no quad and no instance — this is the
    // attachment a world pass *would* have written for a floor covering
    // everything.
    let pixels = drawn(device, queue, lighting, view, width, height, |px, py| {
        let tile_x = bounds.min_x + (px / scale) as i32;
        let tile_y = bounds.min_y + (py / scale) as i32;
        let sub_x = (px % scale) * 127 / scale;
        let sub_y = (py % scale) * 127 / scale;
        [
            tile_x as u16,
            tile_y as u16,
            // A floor **says** it is one, in the stance above the height. It said
            // `Upright` while this comment said "flat ground" — which cost
            // nothing while a stance was only read for a wall's facing, and
            // stopped being free the day a flat surface got a normal of its own
            // and its tile's panels got the right to shadow it. An instrument
            // that does not write what the world pass writes answers about
            // itself. Decisions 27 and 28.
            crate::place::packed_height(0.0, crate::place::Stance::Flat),
            1 | (sub_x as u16) << 2 | (sub_y as u16) << 9,
        ]
    });
    Picture {
        bounds,
        scale,
        width,
        height,
        pixels,
    }
}

/// One run of wall, **unrolled**: how the light falls across its face.
///
/// The second question a plan view cannot answer. A wall's pixels are not on the
/// ground — they are on a vertical plane standing on one edge of a tile — so a
/// picture of the floor says nothing about them, and the defects that live there
/// are their own: a seam between two tiles of one run, a face lit from the wrong
/// side, a gradient that steps at a tile boundary rather than running through it.
///
/// The picture is the face laid flat: **across** is how far along the run, one
/// tile to `scale` pixels, and **down** is height, from `wall.top` at the top of
/// the picture to `z = 0` at the bottom, at the same scale so that a tile of run
/// and a tile of height are the same size. Each pixel is written into the place
/// attachment exactly as `statics.wgsl` would write it for that point of that
/// face — which is what makes this a picture of the shader's answer rather than
/// of a second one.
pub fn elevation(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    lighting: &Lighting,
    view: View,
    wall: Wall,
    scale: u32,
) -> Picture {
    let width = wall.tiles * scale;
    let height = (wall.top.max(1) as u32 * scale) / crate::light::Z_PER_TILE as u32;
    let along_x = matches!(wall.face, Face::North | Face::South);
    let pixels = drawn(device, queue, lighting, view, width, height, |px, py| {
        // How far along the run, in tiles, and the height — the picture's two
        // axes, turned back into a point on the face.
        let along = px as f32 / scale as f32;
        let z = wall.top as f32 * (1.0 - py as f32 / height as f32);
        let (tile, run) = (along.floor() as u16, along.fract());
        // The face's own fraction, held one step of the seven-bit grid inside the
        // tile: decision 16, and the same `INSIDE` clamp `statics.wgsl` applies —
        // a fraction of exactly one names the tile beyond the wall.
        let inside = 126.0 / 127.0;
        let (sub_x, sub_y) = match wall.face {
            Face::North => (run, 0.0),
            Face::South => (run, inside),
            Face::West => (0.0, run),
            Face::East => (inside, run),
        };
        let (tile_x, tile_y) = match along_x {
            true => (wall.from.0 + tile, wall.from.1),
            false => (wall.from.0, wall.from.1 + tile),
        };
        // The stance rides above the height, exactly as `statics.wgsl` writes it
        // — this picture is of a *wall's face*, and a face that did not say which
        // way it looks would be lit from behind. `crate::place::STANCE_SHIFT`.
        let stance = match wall.face {
            Face::North => crate::place::Stance::FaceNorth,
            Face::East => crate::place::Stance::FaceEast,
            Face::South => crate::place::Stance::FaceSouth,
            Face::West => crate::place::Stance::FaceWest,
        };
        [
            tile_x,
            tile_y,
            // Through `packed_height`, which keeps the height's fraction: this
            // picture's whole vertical axis *is* height down a face, so a
            // packing that rounded it to whole units — as this line did, with
            // its own copy of the format — drew the one-unit treads
            // `docs/lighting_height.md` is about, in the instrument meant to
            // show them. An instrument that does not write what the world pass
            // writes answers about itself.
            crate::place::packed_height(z, stance),
            2 | ((sub_x * 127.0) as u16) << 2 | ((sub_y * 127.0) as u16) << 9,
        ]
    });
    Picture {
        // The run, as a rectangle one tile deep: `Picture::tile` is meaningless
        // for an elevation and `at` is about the plan's axes, so what this carries
        // is where the run was rather than a coordinate system.
        bounds: TileBounds {
            min_x: i32::from(wall.from.0),
            max_x: i32::from(wall.from.0)
                + match along_x {
                    true => wall.tiles as i32 - 1,
                    false => 0,
                },
            min_y: i32::from(wall.from.1),
            max_y: i32::from(wall.from.1)
                + match along_x {
                    true => 0,
                    false => wall.tiles as i32 - 1,
                },
        },
        scale,
        width,
        height,
        pixels,
    }
}

/// Which run of wall an [`elevation`] is of.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Wall {
    /// The tile the run starts at — its lowest `x` for a north or south face, its
    /// lowest `y` for an east or west one.
    pub from: (u16, u16),
    /// Which edge of its tiles it stands on, and therefore which way it runs: a
    /// north or south face runs along `+x`, an east or west face along `+y`.
    pub face: Face,
    /// How many tiles of it to draw.
    pub tiles: u32,
    /// The height at the top of the picture. The bottom is `z = 0`.
    pub top: i32,
}

/// Run the blit over a place attachment this caller writes, and read the surface
/// back.
///
/// The half [`draw`] and [`elevation`] share: everything except *where a pixel
/// says it is*, which is the only thing either of them invents.
fn drawn(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    lighting: &Lighting,
    view: View,
    width: u32,
    height: u32,
    place_of: impl Fn(u32, u32) -> [u16; 4],
) -> Vec<u8> {
    let world = crate::blit::world_texture(device, width, height);
    let world_view = world.create_view(&wgpu::TextureViewDescriptor::default());
    let place = crate::place::texture(device, width, height);
    let place_view = place.create_view(&wgpu::TextureViewDescriptor::default());

    // The place attachment the caller describes, texel by texel: `crate::place`'s
    // packing, written here rather than built through `Place` because there is no
    // quad and no instance — this is what a world pass *would* have written for
    // the surface being asked about.
    let mut texels: Vec<u16> = Vec::with_capacity((width * height * 4) as usize);
    for py in 0..height {
        for px in 0..width {
            texels.extend_from_slice(&place_of(px, py));
        }
    }
    // `Kind::Static` pixels — [`elevation`]'s, never [`draw`]'s — no longer
    // carry their own tile in the attachment: `docs/gbuffer.md` step 3 moved
    // it to a row this function has to build itself now, the same as a real
    // static's world pass would have. One row per distinct tile the closure
    // above actually used, keyed by first sight, and the tile each static
    // pixel wrote is rewritten in place to the id that names it.
    let mut face_ids: std::collections::HashMap<(u16, u16), u32> = std::collections::HashMap::new();
    let mut face_rows: Vec<u8> = Vec::new();
    for texel in texels.chunks_exact_mut(4) {
        if texel[3] & 3 != crate::place::Kind::Static as u16 {
            continue;
        }
        let (x, y) = (texel[0], texel[1]);
        let id = *face_ids.entry((x, y)).or_insert_with(|| {
            let id = (face_rows.len() as u64 / crate::sprite::SpriteQuad::STRIDE) as u32;
            crate::sprite::SpriteQuad {
                rect: crate::geometry::Rect {
                    x: 0.0,
                    y: 0.0,
                    width: 0.0,
                    height: 0.0,
                },
                region: crate::atlas::Region {
                    u: 0.0,
                    v: 0.0,
                    du: 0.0,
                    dv: 0.0,
                },
                depth: 0.0,
                hue: 0,
                place: crate::place::Place::land(x, y),
                // No `place_of` closure this function is given ever asks for
                // a corner `Stance` today, so there is never a second half to
                // point at — see `crate::sprite::split_corners` for the real
                // pass's version of this row, which does set it.
                twin: 0,
                // A diagnostic picture is never walked for shadows, so there is
                // no occluder for a row of it to be a point of.
                owner: u32::from(crate::occlusion::OwnerId::NONE.raw()),
            }
            .write(&mut face_rows);
            id
        });
        texel[0] = (id & 0xFFFF) as u16;
        texel[1] = (id >> 16) as u16;
    }
    // `Kind::Land` pixels — [`draw`]'s, never [`elevation`]'s — need the same
    // treatment since `docs/gbuffer.md` step 7: a land pixel's tile is an id
    // into a row this function builds itself now too, the ground half of the
    // static loop above.
    let mut ground_ids: std::collections::HashMap<(u16, u16), u32> = std::collections::HashMap::new();
    let mut ground_rows: Vec<u8> = Vec::new();
    for texel in texels.chunks_exact_mut(4) {
        if texel[3] & 3 != crate::place::Kind::Land as u16 {
            continue;
        }
        let (x, y) = (texel[0], texel[1]);
        let id = *ground_ids.entry((x, y)).or_insert_with(|| {
            let id = (ground_rows.len() as u64 / crate::ground::GroundQuad::STRIDE) as u32;
            crate::ground::GroundQuad {
                x: 0.0,
                y: 0.0,
                corners: [0.0; 4],
                region: crate::atlas::Region {
                    u: 0.0,
                    v: 0.0,
                    du: 0.0,
                    dv: 0.0,
                },
                texmap: None,
                depth: 0.0,
                place: crate::place::Place::land(x, y),
            }
            .write(&mut ground_rows);
            id
        });
        texel[0] = (id & 0xFFFF) as u16;
        texel[1] = (id >> 16) as u16;
    }

    let bytes: Vec<u8> = texels.iter().flat_map(|word| word.to_le_bytes()).collect();
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &place,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &bytes,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(width * 8),
            rows_per_image: Some(height),
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );

    let surface = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("plan"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: crate::blit::WORLD_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let surface_view = surface.create_view(&wgpu::TextureViewDescriptor::default());

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    // The white world, as a clear that is stored: a render target is not a copy
    // destination, so this is the way to fill one.
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("plan world"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: &world_view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::WHITE),
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        multiview_mask: None,
        timestamp_writes: None,
        occlusion_query_set: None,
    });

    let mut lighting = lighting.clone();
    lighting.view = view;
    let mut blit = crate::blit::Blit::new(device, crate::blit::WORLD_FORMAT);
    // No mobile pixels this function ever draws: the dummy always stands in
    // for `mobile_instances`. `face_instances` is the same dummy for `draw`,
    // which only ever writes `Kind::Land`, and the real buffer built above
    // for `elevation`, which writes `Kind::Static` — and `ground_instances` is
    // the mirror of that: the real buffer for `draw`, the dummy for
    // `elevation`.
    let dummy_instances = crate::blit::dummy_instances(device);
    let dummy_mesh_instances = crate::blit::dummy_mesh_instances(device);
    let dummy_ground_instances = crate::blit::dummy_ground_instances(device);
    let face_instances = if face_rows.is_empty() {
        None
    } else {
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("plan face instances"),
            size: face_rows.len() as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&buffer, 0, &face_rows);
        Some(buffer)
    };
    let ground_instances = if ground_rows.is_empty() {
        None
    } else {
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("plan ground instances"),
            size: ground_rows.len() as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&buffer, 0, &ground_rows);
        Some(buffer)
    };
    blit.render(
        device,
        queue,
        &mut encoder,
        crate::blit::Frame {
            target: &surface_view,
            world: &world_view,
            place: &place_view,
            face_instances: face_instances.as_ref().unwrap_or(&dummy_instances),
            mobile_instances: &dummy_instances,
            mesh_instances: &dummy_mesh_instances,
            ground_instances: ground_instances.as_ref().unwrap_or(&dummy_ground_instances),
            zoom: Zoom::ONE,
            rect: crate::blit::ViewportRect {
                x: 0,
                y: 0,
                width,
                height,
            },
        },
        &lighting,
    );
    queue.submit([encoder.finish()]);
    read_back(device, queue, &surface, width, height)
}

/// An RGBA8 texture, as rows with the copy's padding taken off.
///
/// The padding is why this is here rather than in a test: a buffer copy wants
/// rows a multiple of 256 bytes, and a plan view's width is a tile count times a
/// scale — a number nobody should have to choose for the alignment's sake.
fn read_back(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    width: u32,
    height: u32,
) -> Vec<u8> {
    let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let padded = width * 4 + (align - (width * 4) % align) % align;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("plan readback"),
        size: u64::from(padded * height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
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

    let slice = buffer.slice(..);
    slice.map_async(wgpu::MapMode::Read, |result| result.expect("mapping the plan"));
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("the queue drains");
    let mapped = slice.get_mapped_range().expect("the buffer maps");
    let mut pixels = Vec::with_capacity((width * height * 4) as usize);
    for row in 0..height {
        let start = (row * padded) as usize;
        pixels.extend_from_slice(&mapped[start..start + (width * 4) as usize]);
    }
    drop(mapped);
    buffer.unmap();
    pixels
}
