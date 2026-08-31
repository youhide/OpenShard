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
//! shader, the real [`Lighting`] — over a *synthetic* G-buffer that says
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

use crate::camera::{
    TileBounds,
    Zoom,
};
use crate::debug::View;
use crate::facing::Face;
use crate::light::Lighting;
use crate::occlusion::{
    self,
    Cell,
};

/// One instance row an instrument built, and what it claimed about the world.
///
/// **A picture cannot be asked what attachment it was drawn from**, and that is
/// not a theoretical gap: [`elevation`] stamped
/// [`occlusion::OwnerId::NONE`] into every row it built for three phases, with
/// two green tests drawn through it the whole time, because the one field that
/// was wrong is the one no pixel shows. `docs/lighting_rebuild.md`'s backlog
/// asks for a gate on it, and a gate needs the claim in a form it can read.
///
/// One of these per row [`drawn`] builds — that is, one per distinct tile of
/// each kind, keyed by first sight, which is exactly what a world pass would
/// have uploaded.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Named {
    /// The tile the row is for.
    pub tile:  (u16, u16),
    /// Which pass's row it is: [`crate::place::Kind::Static`] for a face row,
    /// [`crate::place::Kind::Land`] for a ground one. Nothing else has a row
    /// here — see [`drawn`], where a kind with no instance buffer takes row
    /// zero and is never named.
    pub kind:  crate::place::Kind,
    /// **Which occluder of that tile the row named**, straight out of the grid
    /// — or [`occlusion::OwnerId::NONE`] for a surface that is a point of no
    /// occluder, which is the ground's honest answer and was the elevation's
    /// wrong one.
    pub owner: occlusion::OwnerId,
}

/// One drawn plan, and what it was drawn of.
///
/// RGBA rows, top-left first, `width * 4` bytes each — the layout the readback
/// produces and the one [`Picture::png`] writes out.
#[derive(Clone, Debug)]
pub struct Picture {
    /// The tiles it covers, inclusive.
    pub bounds: TileBounds,
    /// How many pixels one tile is, on both axes.
    pub scale:  u32,
    /// Its width in pixels: `bounds.width() * scale`.
    pub width:  u32,
    /// Its height.
    pub height: u32,
    /// The pixels, RGBA8.
    pub pixels: Vec<u8>,
    /// The instance rows the pixels were drawn from — see [`Named`].
    ///
    /// Carried rather than discarded because it is the only place an
    /// instrument's claim about the world is stated, and a picture is not one:
    /// `tests/attachment.rs` compares these against the grid.
    pub named:  Vec<Named>,
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
            if cell.edges == occlusion::Edges::NONE {
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
                (occlusion::Edges::NORTH, (left, top), (right, top)),
                (occlusion::Edges::SOUTH, (left, bottom), (right, bottom)),
                (occlusion::Edges::WEST, (left, top), (left, bottom)),
                (occlusion::Edges::EAST, (right, top), (right, bottom)),
            ] {
                if !cell.edges.contains(edge) {
                    continue;
                }
                self.line(from, to, color);
                let inward = match edge {
                    occlusion::Edges::NORTH => (0, 1),
                    occlusion::Edges::SOUTH => (0, -1),
                    occlusion::Edges::WEST => (1, 0),
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
    // A plan view is all ground, and `drawn` asks this only where it builds a
    // *face* row — so on this instrument it is never asked at all, and the
    // closure says so by refusing rather than by answering
    // [`crate::occlusion::OwnerId::NONE`].
    //
    // It answered `NONE` until this line, and that is the shape of the defect
    // `elevation` shipped for three phases: a constant that is right only
    // because nothing reads it, and that goes on being returned the day
    // something does. A plan view that grew a static would then be drawing a
    // fragment that is a point of nothing — silently, since no pixel shows the
    // field. This way it stops, and whoever added the static says what it is a
    // point of.
    let owner_of = |tile| unreachable!("a plan view drew a static on {tile:?} and named no owner");
    let (pixels, named) = drawn(
        device,
        queue,
        lighting,
        view,
        (width, height),
        owner_of,
        |px, py| {
            crate::gbuffer::Fragment {
                tile:   (
                    (bounds.min_x + (px / scale) as i32) as u16,
                    (bounds.min_y + (py / scale) as i32) as u16,
                ),
                sub:    (
                    (px % scale) as f32 / scale as f32,
                    (py % scale) as f32 / scale as f32,
                ),
                z:      0.0,
                kind:   crate::place::Kind::Land,
                // A floor **says** it is one, in the stance above the height. It said
                // `Upright` while this comment said "flat ground" — which cost
                // nothing while a stance was only read for a wall's facing, and
                // stopped being free the day a flat surface got a normal of its own
                // and its tile's panels got the right to shadow it. An instrument
                // that does not write what the world pass writes answers about
                // itself. Decisions 27 and 28.
                stance: crate::place::Stance::Flat,
                // And a point of no solid: this view's every pixel is the
                // ground, which is no occluder and exempt from nothing — the
                // same answer `ground.wesl` writes in a real frame.
                solid:  None,
            }
        },
    );
    Picture {
        bounds,
        scale,
        width,
        height,
        pixels,
        named,
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
    // **And which occluder of its own tile each pixel of the run is a point of.**
    //
    // This was [`crate::occlusion::OwnerId::NONE`] for every row, under a comment
    // saying a diagnostic picture is never walked for shadows — and `View::Flames`
    // is a walk, so the two tests this instrument exists for were measuring a face
    // that is a point of nothing: exempt from nothing, shadowed by its own panel.
    // What stood in for the exemption was `light::same_run`'s row arithmetic, and
    // that is the whole reason that function still reads as load-bearing.
    // `docs/lighting_rebuild.md`'s backlog, the same finding as the two fixtures in
    // `tests/lighting.rs` that never called `Spot::part_of`.
    //
    // **The owner is the row's now and no more than that**: the shader stopped
    // narrowing it to a panel per fragment when the solid came to ride in the
    // position plane (`solid_format.wesl`), so what this picture is a face of is
    // stated outright below, by `Occlusion::id_facing`, and this is what
    // `tests/attachment.rs` reads back off the row.
    let owner_of = |tile: (u16, u16)| {
        lighting
            .occlusion
            .owner_at(i32::from(tile.0), i32::from(tile.1), wall.of.z, wall.of.graphic)
    };
    let (pixels, named) = drawn(
        device,
        queue,
        lighting,
        view,
        (width, height),
        owner_of,
        |px, py| {
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
            // Through [`crate::gbuffer::Fragment`], which keeps the height's
            // fraction and the tile's: this picture's whole vertical axis *is*
            // height down a face, so a packing that rounded it to whole units — as
            // this closure did, with its own copy of the format — drew the one-unit
            // treads `docs/lighting_height.md` is about, in the instrument meant to
            // show them. An instrument that does not write what the world pass
            // writes answers about itself.
            crate::gbuffer::Fragment {
                tile: (tile_x, tile_y),
                sub: (sub_x, sub_y),
                z,
                kind: crate::place::Kind::Static,
                stance,
                // **And which solid that face is**, which the shader used to
                // find for itself out of the owner above and this stance —
                // `blit.wesl`'s late `own_solid`. It reads the name off the
                // position plane now, so the instrument states it, which is
                // the same asymmetry `owner_of` already carries: a picture of
                // one face of a run knows exactly which panel it drew.
                solid: lighting.occlusion.id_facing(
                    i32::from(tile_x),
                    i32::from(tile_y),
                    wall.of,
                    match wall.face {
                        Face::North => crate::occlusion::Edges::NORTH,
                        Face::East => crate::occlusion::Edges::EAST,
                        Face::South => crate::occlusion::Edges::SOUTH,
                        Face::West => crate::occlusion::Edges::WEST,
                    },
                ),
            }
        },
    );
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
        named,
    }
}

/// Which run of wall an [`elevation`] is of.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Wall {
    /// The tile the run starts at — its lowest `x` for a north or south face, its
    /// lowest `y` for an east or west one.
    pub from:  (u16, u16),
    /// Which edge of its tiles it stands on, and therefore which way it runs: a
    /// north or south face runs along `+x`, an east or west face along `+y`.
    pub face:  Face,
    /// How many tiles of it to draw.
    pub tiles: u32,
    /// The height at the top of the picture. The bottom is `z = 0`.
    pub top:   i32,
    /// **The static the run is made of** — the `z` it stands at and its graphic,
    /// which is the key [`crate::occlusion::Occlusion::owner_at`] turns into the
    /// number the grid gave that static on each of the run's tiles.
    ///
    /// Stated by the caller rather than searched for, and the whole of why: a
    /// tile carries several occluders and a run of wall is one of them. Picking
    /// "the one that looks like a wall" out of a cell would be this instrument
    /// deciding what it is a picture *of* — which is the caller's fact, and the
    /// caller is drawing a run it built.
    pub of:    crate::occlusion::Owner,
}

/// Run the blit over a G-buffer this caller writes, and read the surface back.
///
/// The half [`draw`] and [`elevation`] share: everything except *where a pixel
/// says it is*, which is the only thing either of them invents.
///
/// `owner_of` is asked once per distinct tile a *static* fragment names — where
/// the instance row is built, which is the one place an owner can be written —
/// and never for the ground, which is a point of no occluder by construction.
///
/// `size` is the picture's, as one pair: the two are never chosen apart, and
/// splitting them was what put this over clippy's argument count.
///
/// Back come the pixels **and the rows they were drawn from** — [`Named`], one
/// per row this builds, which is the only statement of what the instrument
/// claimed about the world and the thing `tests/attachment.rs` gates.
fn drawn(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    lighting: &Lighting,
    view: View,
    size: (u32, u32),
    owner_of: impl Fn((u16, u16)) -> crate::occlusion::OwnerId,
    place_of: impl Fn(u32, u32) -> crate::gbuffer::Fragment,
) -> (Vec<u8>, Vec<Named>) {
    let (width, height) = size;
    let world = crate::blit::world_texture(device, width, height);
    let world_view = world.create_view(&wgpu::TextureViewDescriptor::default());
    let gbuffer = crate::gbuffer::Gbuffer::new(device, width, height);
    let views = gbuffer.views();

    // The G-buffer the caller describes, texel by texel — every plane, through
    // [`crate::gbuffer::Fragment`] rather than packed here, so that this
    // instrument writes what a world pass writes and not a second reading of
    // the format. There is no quad: this *is* what a pass would have left for
    // the surface being asked about.
    //
    // Gathered whole before anything is packed, because **an id is not a fact a
    // fragment knows**. A world pass has one per instance from the rasteriser;
    // here it is a row number, and a row number cannot be handed out until every
    // fragment that wants one has been seen. So the closure describes surfaces
    // by their tile and the two loops below turn tiles into rows —
    // `docs/gbuffer.md` step 3 for a static, step 7 for the ground, both of
    // which moved a tile out of the attachment and into an instance buffer this
    // function has to build itself, the same as a real world pass would have.
    let fragments: Vec<crate::gbuffer::Fragment> = (0..height)
        .flat_map(|py| (0..width).map(move |px| (px, py)))
        .map(|(px, py)| place_of(px, py))
        .collect();

    // One row per distinct tile each kind actually used, keyed by first sight.
    let mut face_ids: rustc_hash::FxHashMap<(u16, u16), u32> = rustc_hash::FxHashMap::default();
    let mut face_rows: Vec<u8> = Vec::new();
    let mut ground_ids: rustc_hash::FxHashMap<(u16, u16), u32> = rustc_hash::FxHashMap::default();
    let mut ground_rows: Vec<u8> = Vec::new();
    let mut ids: Vec<u32> = Vec::with_capacity(fragments.len());
    // What each of those rows said about the world, in the order they were
    // built. The picture carries it out — see [`Named`].
    let mut named: Vec<Named> = Vec::new();
    for fragment in &fragments {
        let tile = fragment.tile;
        // A kind with no instance buffer of its own here takes row zero and
        // nothing reads it: only these two are ever asked for a tile, and the
        // closures this function is given produce no others.
        let id = match fragment.kind {
            // `Kind::Static` — [`elevation`]'s pixels, never [`draw`]'s.
            crate::place::Kind::Static => {
                *face_ids.entry(tile).or_insert_with(|| {
                    let id = (face_rows.len() as u64 / crate::sprite::SpriteQuad::STRIDE) as u32;
                    let owner = owner_of(tile);
                    named.push(Named {
                        tile,
                        kind: crate::place::Kind::Static,
                        owner,
                    });
                    crate::sprite::SpriteQuad {
                        rect:    crate::geometry::Rect {
                            x:      0.0,
                            y:      0.0,
                            width:  0.0,
                            height: 0.0,
                        },
                        region:  crate::atlas::Region {
                            u:  0.0,
                            v:  0.0,
                            du: 0.0,
                            dv: 0.0,
                        },
                        depth:   0.0,
                        hue:     0,
                        place:   crate::place::Place::land(tile.0, tile.1),
                        // No `place_of` closure this function is given ever asks for
                        // a corner `Stance` today, so there is never a second half to
                        // point at — see `crate::sprite::split_corners` for the real
                        // pass's version of this row, which does set it.
                        twin:    0,
                        // **Which occluder of this tile the picture is of**, from the
                        // caller — see [`elevation`]'s `owner_of`, and the comment
                        // there for what this said before and what it cost.
                        owner:   u32::from(owner.raw()),
                        volumes: crate::impostor::Range::default(),
                    }
                    .write(&mut face_rows);
                    id
                })
            }
            // `Kind::Land` — [`draw`]'s pixels, never [`elevation`]'s.
            crate::place::Kind::Land => {
                *ground_ids.entry(tile).or_insert_with(|| {
                    let id = (ground_rows.len() as u64 / crate::ground::GroundQuad::STRIDE) as u32;
                    // A [`crate::ground::GroundQuad`] has no owner field at all, and
                    // that is the honest exception rather than a fourth place a
                    // number could be forgotten: `occlusion::Builder` is only ever
                    // handed statics and ground items — see `occlusion::place` — so
                    // no land tile is ever a solid, and a field that could only hold
                    // `NONE` is a field a later writer could get wrong.
                    named.push(Named {
                        tile,
                        kind: crate::place::Kind::Land,
                        owner: crate::occlusion::OwnerId::NONE,
                    });
                    crate::ground::GroundQuad {
                        x:       0.0,
                        y:       0.0,
                        corners: [0.0; 4],
                        region:  crate::atlas::Region {
                            u:  0.0,
                            v:  0.0,
                            du: 0.0,
                            dv: 0.0,
                        },
                        texmap:  None,
                        depth:   0.0,
                        place:   crate::place::Place::land(tile.0, tile.1),
                    }
                    .write(&mut ground_rows);
                    id
                })
            }
            crate::place::Kind::Nothing | crate::place::Kind::Mobile => 0,
        };
        ids.push(fragment.ids(id));
    }
    let positions: Vec<f32> = fragments.iter().flat_map(|f| f.position()).collect();
    let normals: Vec<u32> = fragments.iter().map(|f| f.normal()).collect();

    let upload = |texture: &wgpu::Texture, bytes: &[u8], stride: u32| {
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
                bytes_per_row:  Some(width * stride),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
    };
    let bytes: Vec<u8> = ids.iter().flat_map(|word| word.to_le_bytes()).collect();
    upload(gbuffer.ids(), &bytes, 4);
    let bytes: Vec<u8> = positions.iter().flat_map(|value| value.to_le_bytes()).collect();
    upload(gbuffer.position(), &bytes, 16);
    let bytes: Vec<u8> = normals.iter().flat_map(|word| word.to_le_bytes()).collect();
    upload(gbuffer.normal(), &bytes, 4);

    let surface = device.create_texture(&wgpu::TextureDescriptor {
        label:           Some("plan"),
        size:            wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count:    1,
        dimension:       wgpu::TextureDimension::D2,
        format:          crate::blit::WORLD_FORMAT,
        usage:           wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats:    &[],
    });
    let surface_view = surface.create_view(&wgpu::TextureViewDescriptor::default());

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    // The white world, as a clear that is stored: a render target is not a copy
    // destination, so this is the way to fill one.
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("plan world"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view:           &world_view,
            depth_slice:    None,
            resolve_target: None,
            ops:            wgpu::Operations {
                load:  wgpu::LoadOp::Clear(wgpu::Color::WHITE),
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
            label:              Some("plan face instances"),
            size:               face_rows.len() as u64,
            usage:              wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&buffer, 0, &face_rows);
        Some(buffer)
    };
    let ground_instances = if ground_rows.is_empty() {
        None
    } else {
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label:              Some("plan ground instances"),
            size:               ground_rows.len() as u64,
            usage:              wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
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
            target:           &surface_view,
            world:            &world_view,
            gbuffer:          &views,
            face_instances:   face_instances.as_ref().unwrap_or(&dummy_instances),
            item_instances:   &dummy_instances,
            mobile_instances: &dummy_instances,
            mesh_instances:   &dummy_mesh_instances,
            ground_instances: ground_instances.as_ref().unwrap_or(&dummy_ground_instances),
            zoom:             Zoom::ONE,
            rect:             crate::blit::ViewportRect {
                x: 0,
                y: 0,
                width,
                height,
            },
        },
        &lighting,
    );
    queue.submit([encoder.finish()]);
    let whole = crate::blit::ViewportRect {
        x: 0,
        y: 0,
        width,
        height,
    };
    (crate::dump::read_rect(device, queue, &surface, whole), named)
}
