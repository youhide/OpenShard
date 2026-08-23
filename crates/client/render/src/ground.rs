//! Turning a patch of map into the quads that draw it.
//!
//! This is the whole CPU side of the ground: read the land cells the camera can
//! see, project each one, and look its sprite up in the atlas. No GPU type
//! appears here, so it can be checked by counting and comparing numbers.

use std::collections::BTreeSet;

use openshard_map::map::{LandCell, WorldMap};
use openshard_protocol::wire::Graphic;
use openshard_protocol::world::Point;

use crate::atlas::{LandAtlas, Region, TexmapAtlas};
use crate::camera::{Camera, TileBounds};
use crate::cutaway::Cutaway;
use crate::depth;

/// One ground quad: where it goes, how its corners stand, and what to sample.
///
/// The position is the diamond's centre **at height zero**. Height is not folded
/// in here because there is no single height to fold: a tile is a patch stretched
/// over four corners, and only the shader knows which corner a vertex is.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct GroundQuad {
    /// The diamond's centre, in viewport pixels, ignoring height. Fractional
    /// never happens, but the GPU wants floats and converting once here keeps
    /// the buffer writer trivial.
    pub x: f32,
    /// The same, downwards.
    pub y: f32,
    /// The heights of the tile's four corners, in the order the shader indexes
    /// them: `(x, y)`, `(x+1, y)`, `(x, y+1)`, `(x+1, y+1)` — index `a + 2*b`
    /// for a unit-quad corner `(a, b)`. On screen that reads top, right, left,
    /// bottom.
    ///
    /// Floats for the same reason as the position; every value came from an
    /// `i8` and converts back exactly.
    pub corners: [f32; 4],
    /// Where its sprite lives in the land atlas.
    pub region: Region,
    /// Where its square texture lives in the texture atlas, if it has one.
    ///
    /// Absent is ordinary and means what it says: three quarters of the land
    /// index has no texture at all, and such a tile is drawn from `region` at
    /// whatever shape its corners make. The shader is told by a zero size, which
    /// no real region can have.
    pub texmap: Option<Region>,
    /// What hides it and what it hides: smaller is nearer. See [`crate::depth`].
    ///
    /// Ground is drawn before the statics, so within one frame this decides
    /// nothing about the ground itself — tiles rarely overlap tiles. It decides
    /// everything about the *other* pass: a hillside has to cover the wall
    /// standing behind it, and the pass order alone would put every static in
    /// front of every tile.
    pub depth: f32,
    /// Which tile this is, for the lighting pass.
    ///
    /// [`Place::land`](crate::place::Place::land), and the height in it is not
    /// read: the shader lifts each corner by its own, so the height a pixel of a
    /// hillside carries is the interpolated one rather than the tile's base. See
    /// [`crate::place`].
    pub place: crate::place::Place,
}

impl GroundQuad {
    /// Whether every corner has the same height.
    ///
    /// A flat quad is self-contained map art. A slope, by contrast, derives
    /// its surface from adjacent map cells; callers that compose one bounded
    /// block can use this distinction to keep that shared surface live.
    pub fn is_flat(self) -> bool {
        self.corners.iter().all(|height| *height == self.corners[0])
    }

    /// Bytes one quad occupies in the instance buffer.
    ///
    /// Fifteen floats and the two words of [`crate::place::Place`]: position,
    /// the four corner heights, the land region, the texture region, the depth
    /// and the tile. Written by hand rather than cast from a struct —
    /// `bytemuck`'s derive emits `unsafe impl`, and this workspace denies
    /// `unsafe_code` outright. Seventeen `to_le_bytes` is a cheaper price than
    /// an exception to that rule.
    pub const STRIDE: u64 = 17 * 4;

    /// Append this quad to an instance buffer.
    pub fn write(&self, out: &mut Vec<u8>) {
        // A tile with no texture writes a region of zero size. The shader tests
        // the size rather than the position, because (0, 0) is a legitimate
        // corner of the atlas and a zero *extent* is not a texture anything
        // could be sampled from.
        let texmap = self.texmap.unwrap_or(Region {
            u: 0.0,
            v: 0.0,
            du: 0.0,
            dv: 0.0,
        });
        for value in [
            self.x,
            self.y,
            self.corners[0],
            self.corners[1],
            self.corners[2],
            self.corners[3],
            self.region.u,
            self.region.v,
            self.region.du,
            self.region.dv,
            texmap.u,
            texmap.v,
            texmap.du,
            texmap.dv,
            self.depth,
        ] {
            out.extend_from_slice(&value.to_le_bytes());
        }
        for word in self.place.packed() {
            out.extend_from_slice(&word.to_le_bytes());
        }
    }
}

/// Every distinct land graphic the camera can see.
///
/// Called before building the atlas, which is why it is separate from
/// [`collect`]: the atlas has to exist before a quad can be given a region.
pub fn visible_graphics(map: &WorldMap, camera: &Camera) -> BTreeSet<Graphic> {
    let mut seen = BTreeSet::new();
    graphics_in(map, camera.visible_tiles(), &mut seen);
    seen
}

/// Every distinct land graphic on the cells of one rectangle, added to `out`.
///
/// The same walk [`visible_graphics`] does, over a rectangle somebody else
/// chose. That somebody is the caller growing an atlas: what a frame needs that
/// the atlas does not already hold is a question about the *edge* the camera
/// crossed, and [`TileBounds::difference`] is what turns the viewport into the
/// two or three thin bands that crossed it. Walking the whole rectangle instead
/// is nine thousand cells at 1080p, on every frame, to discover that a camera
/// which moved one tile introduced one row.
///
/// Accumulating into `out` rather than returning a set, because the caller has
/// several bands and one atlas.
pub fn graphics_in(map: &WorldMap, bounds: TileBounds, out: &mut BTreeSet<Graphic>) {
    for_each_cell_in(map, bounds, |_, _, cell| {
        out.insert(Graphic(cell.tile.0));
    });
}

/// The quads for everything visible, in the order they must be drawn.
///
/// A tile whose graphic is not in the atlas is dropped: either the client ships
/// no art for it, or the atlas was built for a different camera. Both are
/// "nothing to draw here", and neither is worth failing a frame over.
///
/// `cutaway` hides the ground above the player's head, which is the one case
/// the client cuts land for at all: standing in a cave under a hill, the hill's
/// own tiles would otherwise be drawn over the cave. A roof does *not* take the
/// floor with it — see [`crate::cutaway`], where that is the client's odd last
/// line and not an omission here.
pub fn collect(
    map: &WorldMap,
    camera: &Camera,
    atlas: &LandAtlas,
    texmaps: &TexmapAtlas,
    cutaway: &Cutaway,
) -> Vec<GroundQuad> {
    collect_with_interior(map, camera, atlas, texmaps, cutaway, None)
}

/// [`collect`] with a resolved building frame layered over the global cutaway.
pub fn collect_with_interior(
    map: &WorldMap,
    camera: &Camera,
    atlas: &LandAtlas,
    texmaps: &TexmapAtlas,
    cutaway: &Cutaway,
    interior: Option<&crate::interiors::InteriorFrame>,
) -> Vec<GroundQuad> {
    collect_in_with_interior(
        map,
        camera,
        camera.visible_tiles(),
        atlas,
        texmaps,
        cutaway,
        interior,
    )
}

/// The ground quads on one caller-selected tile rectangle.
///
/// The detailed renderer uses [`Camera::visible_tiles`] through [`collect`].
/// Cached map-block composition uses this narrower form so an 8×8 block can be
/// built without walking every tile currently visible to the camera.  The
/// rectangles have the same inclusive, map-clamped semantics as the camera's
/// own bounds; only their source differs.
pub fn collect_in(
    map: &WorldMap,
    camera: &Camera,
    bounds: TileBounds,
    atlas: &LandAtlas,
    texmaps: &TexmapAtlas,
    cutaway: &Cutaway,
) -> Vec<GroundQuad> {
    collect_in_with_interior(map, camera, bounds, atlas, texmaps, cutaway, None)
}

/// [`collect_in`] with an optional building-cell gate.
pub fn collect_in_with_interior(
    map: &WorldMap,
    camera: &Camera,
    bounds: TileBounds,
    atlas: &LandAtlas,
    texmaps: &TexmapAtlas,
    cutaway: &Cutaway,
    interior: Option<&crate::interiors::InteriorFrame>,
) -> Vec<GroundQuad> {
    let (eye_x, eye_y) = camera.eye_tile();
    let base = depth::base_for(eye_x, eye_y);
    let Some((xs, ys)) = bounds.clamp_to(map.width(), map.height()) else {
        return Vec::new();
    };
    let (min_x, max_x) = (i32::from(*xs.start()), i32::from(*xs.end()));
    let (min_y, max_y) = (i32::from(*ys.start()), i32::from(*ys.end()));
    // The whole rectangle, in the order the map is stored, before the walk
    // below reads it in the order the screen wants — see [`LandWindow`].
    let land = LandWindow::gather(map, min_x, min_y, max_x, max_y);
    // Sized from the rectangle rather than grown: the count is known exactly
    // (minus whatever is off-map or cut away), and a far-zoom frame would
    // otherwise reallocate a list of tens of thousands of quads fifteen times
    // on its way up.
    let mut quads = Vec::with_capacity(((max_x - min_x + 1) * (max_y - min_y + 1)).max(0) as usize);
    let mut diagonal: Vec<(depth::Order, GroundQuad)> = Vec::new();

    // `Order`'s primary key is `x + y`, so walking the rectangle by diagonals
    // already gives the global depth order. The old whole-frame stable sort did
    // useful work only inside one diagonal, where terrain heights differ. Keep
    // that short stable sort: y grows in the same order as the old row-major
    // walk, therefore equal priorities remain byte-for-byte deterministic.
    for tile in min_x + min_y..=max_x + max_y {
        diagonal.clear();
        let first_y = min_y.max(tile - max_x);
        let last_y = max_y.min(tile - min_x);
        for y in first_y..=last_y {
            let x = tile - y;
            let Some(cell) = land.at(x, y) else {
                continue;
            };
            if !cutaway.shows_land(cell.z) {
                continue;
            }
            let corners = land.corners(x, y, cell.z);
            let (x, y) = (x as u16, y as u16);
            if !interior.is_none_or(|frame| frame.shows_at(Point::new(x, y, cell.z))) {
                continue;
            }
            let at = camera.to_screen(Point::new(x, y, 0));
            let Some(region) = atlas.region(Graphic(cell.tile.0)) else {
                continue;
            };
            // `None` here is not a failure to find anything: most land graphics have
            // no texture, and the shader draws those from the art.
            let texmap = texmaps.region(Graphic(cell.tile.0));
            // Height deliberately left at zero: the shader lifts each corner by its
            // own, and folding a representative height in here would count one of
            // them twice.
            // Where this tile sorts against everything else in the frame, statics
            // included: `x + y` down the screen, and the client's own land priority
            // — the tile's average height, less two — up it. The subtraction is
            // what puts ground under the things standing on it; see `crate::depth`.
            let order = depth::Order {
                tile,
                priority_z: depth::land_priority_z(corners.map(|z| z as i32)),
            };
            diagonal.push((
                order,
                GroundQuad {
                    x: at.x as f32,
                    y: at.y as f32,
                    corners,
                    region,
                    texmap,
                    depth: order.to_depth(base),
                    place: crate::place::Place::land(x, y),
                },
            ));
        }
        diagonal.sort_by_key(|(order, _)| *order);
        quads.extend(diagonal.drain(..).map(|(_, quad)| quad));
    }
    quads
}

/// The land of one tile rectangle, copied out of the map row-major.
///
/// **A copy, and it is what the ground pass costs.** [`WorldMap`] stores its cells in
/// 8×8 blocks laid out column-major (see `WorldMap::cell_index`), so two tiles that
/// are neighbours on screen can be hundreds of kilobytes apart in memory — and
/// the walk below is by *anti-diagonal*, which steps `(x + 1, y - 1)` and so
/// leaves its block on almost every tile. Each of those is a cache miss, and a
/// tile takes four of them: its own cell and the three neighbours its corner
/// heights read.
///
/// Read once in the order the map is laid out and the whole visible rectangle
/// is a few thousand sequential lines instead of a hundred thousand scattered
/// ones, into a buffer small enough to stay in L2 for the walk that follows —
/// 26,732 tiles at the widest zoom is about 160 KB here.
///
/// It holds no policy: [`Self::at`] answers exactly what [`WorldMap::land`] would and
/// [`Self::corners`] exactly what [`WorldMap::land_corners`] would, off-map edges
/// included. Nothing about the picture changes.
struct LandWindow {
    /// The north-west tile of the rectangle held.
    min_x: i32,
    min_y: i32,
    /// Its extent in tiles. One column and one row wider than the tiles drawn
    /// from it, because a tile's corner heights are its east and south
    /// neighbours' — clamped where that fringe would leave the map.
    width: usize,
    height: usize,
    /// Row-major over that extent. `None` is a tile the map has no cell for,
    /// which is off its edge.
    cells: Vec<Option<LandCell>>,
}

impl LandWindow {
    /// Copy the inclusive rectangle `(min_x, min_y)..=(max_x, max_y)`, plus the
    /// one east column and south row the corner heights read.
    ///
    /// The gather is row-major on purpose: within one 8×8 block a row of tiles
    /// is contiguous, and eight consecutive rows revisit the same handful of
    /// blocks, so the whole read stays in L1 as it sweeps.
    fn gather(map: &WorldMap, min_x: i32, min_y: i32, max_x: i32, max_y: i32) -> Self {
        let width = (max_x - min_x + 2).max(0) as usize;
        let height = (max_y - min_y + 2).max(0) as usize;
        let mut cells = Vec::with_capacity(width * height);
        for y in 0..height {
            let start = cells.len();
            // The tiles left of the facet's first column. The camera does not
            // know where the map ends, so the bounds may be negative; `resize`
            // is what puts the `None`s there, and the same call pads the far
            // end of the row below.
            let leading = (-min_x).clamp(0, width as i32) as usize;
            cells.resize(start + leading, None);
            // `u16::try_from` rather than a cast, and once per row rather than
            // once per tile: a wrapping cast would fetch a row from the far
            // side of the map. Past the coordinate space either way, there is
            // no cell — which is what a row past the facet's southern edge
            // gets from `land_in_row` too.
            let row = u16::try_from(min_y + y as i32).ok();
            // `leading == width` is a row that ends before the map begins.
            // There is nothing left of it to ask about, and the ask would not
            // be free: `from_x` would be the map's own first column and the
            // walk would run a whole facet row for cells the `resize` below
            // drops.
            let from_x = match leading < width {
                true => u16::try_from(min_x + leading as i32).ok(),
                false => None,
            };
            if let (Some(row), Some(from_x)) = (row, from_x) {
                // One step east per tile rather than a block index derived per
                // tile, and it stops at the facet's eastern edge on its own.
                let to_x = u16::try_from(min_x.saturating_add(width as i32) - 1).unwrap_or(u16::MAX);
                cells.extend(map.land_in_row(row, from_x, to_x).map(Some));
            }
            // Whatever the row did not reach: the eastern edge of the facet,
            // or the whole row when it had none at all.
            cells.resize(start + width, None);
        }
        Self {
            min_x,
            min_y,
            width,
            height,
            cells,
        }
    }

    /// The cell at a map tile, or `None` off the map or outside the window.
    fn at(&self, x: i32, y: i32) -> Option<LandCell> {
        let (column, row) = (x - self.min_x, y - self.min_y);
        if column < 0 || row < 0 || column as usize >= self.width || row as usize >= self.height {
            return None;
        }
        self.cells[row as usize * self.width + column as usize]
    }

    /// [`corner_heights`] out of the window instead of the map.
    ///
    /// The fallback to `own` is [`WorldMap::land_corners`]'s own, for its own
    /// reason: a neighbour off the map has no height to average, so the tile
    /// stands level along that edge rather than falling to zero.
    fn corners(&self, x: i32, y: i32, own: i8) -> [f32; 4] {
        let at = |x: i32, y: i32| self.at(x, y).map_or(own, |cell| cell.z);
        [own, at(x + 1, y), at(x, y + 1), at(x + 1, y + 1)].map(f32::from)
    }
}

/// The heights of a tile's four corners, in [`GroundQuad::corners`] order.
///
/// [`WorldMap::land_corners`] as floats, and the walk itself lives there because the
/// *server* reads the same four numbers: what a tile's corners are is a fact
/// about the map, and the height a body stands at on it — the average — has to
/// be one formula on both sides of the wire or every step up a slope
/// rubber-bands.
///
/// `own` stands in for a tile the map has no cell for at all, which is off its
/// edge: there is nothing to average and nothing to draw, and the caller here
/// has a cell in hand by construction.
///
/// Public because it is not only the ground pass's: a tile's four corners are
/// what say how high the tile *is*, and [`crate::cutaway`] has to ask that of
/// the tile the player is standing on before it can decide what to stop
/// drawing.
pub fn corner_heights(map: &WorldMap, x: u16, y: u16, own: i8) -> [f32; 4] {
    map.land_corners(x, y).unwrap_or([own; 4]).map(f32::from)
}

/// Walk a rectangle, clamped to the map, calling back for each cell.
///
/// The clamp is why the bounds may be negative: the edge of the world is the
/// map's fact, not the camera's, and a camera that knew the map's size would
/// have to be rebuilt whenever the facet changed.
fn for_each_cell_in(
    map: &WorldMap,
    bounds: TileBounds,
    mut each: impl FnMut(u16, u16, openshard_map::map::LandCell),
) {
    let Some((xs, ys)) = bounds.clamp_to(map.width(), map.height()) else {
        return;
    };

    // The clamp already put both ranges inside the facet, so one stepped row
    // hands back exactly one cell per column of `xs` — see [`WorldMap::land_in_row`],
    // which is where the walk order lives now.
    let (from_x, to_x) = (*xs.start(), *xs.end());
    for y in ys {
        for (x, cell) in xs.clone().zip(map.land_in_row(y, from_x, to_x)) {
            each(x, y, cell);
        }
    }
}

#[cfg(test)]
mod tests {
    use openshard_map::grid::BlockExtent;
    use openshard_map::map::LandCell;
    use openshard_uofiles::color::Color16;
    use openshard_uofiles::image::Image;

    use super::*;

    /// The land graphic the fixture map is made of, and which the atlas carries.
    const GRASS: Graphic = Graphic(3);
    /// One the atlas deliberately does not carry — a client that ships no art
    /// for a tile the map names is ordinary, and the frame must survive it.
    const MISSING: Graphic = Graphic(9);

    /// A facet small enough to build in a test, rough enough that no two corners
    /// agree by accident, and scattered with tiles the atlas has no art for.
    ///
    /// Eight blocks each way, so 64x64 tiles. The heights are a fixed pattern
    /// rather than random: this whole file is arithmetic, and a fixture that
    /// changed between runs would make every count below a different number.
    fn hillside() -> WorldMap {
        WorldMap::from_blocks(BlockExtent { wide: 8, down: 8 }, |x, y| LandCell {
            tile: openshard_tiles::LandTileId(if (x + y).is_multiple_of(17) {
                MISSING.0
            } else {
                GRASS.0
            }),
            z: (((i32::from(x) * 3 + i32::from(y) * 7) % 41) - 20) as i8,
        })
    }

    /// An atlas holding art for [`GRASS`] alone.
    ///
    /// The picture is one flat colour: nothing here reads a pixel, and what the
    /// shader does with them is `tests/frame.rs`'s subject and needs a GPU.
    fn grass_atlas() -> LandAtlas {
        let side = usize::from(openshard_uofiles::art::LAND_TILE_SIZE);
        let image = Image::new(
            openshard_uofiles::art::LAND_TILE_SIZE,
            openshard_uofiles::art::LAND_TILE_SIZE,
            vec![Color16(0x7FFF); side * side],
        );
        LandAtlas::pack([(GRASS, image)]).expect("one sprite always fits")
    }

    /// A camera on the middle of [`hillside`], in a viewport of a size the tile
    /// arithmetic does not divide evenly.
    fn camera() -> Camera {
        Camera::new(Point::new(32, 32, 0), 200, 160)
    }

    /// Every cell the camera can see and the atlas has art for becomes one quad,
    /// and nothing else does.
    ///
    /// This is the visible set, stated as a set rather than a count: a count is
    /// green for a walk that draws the wrong tiles as long as it draws the right
    /// number of them. The expected side is built from [`Camera::visible_tiles`]
    /// and [`TileBounds::clamp_to`], which is what `collect` is *supposed* to
    /// walk — so what this pins is that it walks all of it, none of it twice, and
    /// nothing outside it.
    #[test]
    fn every_visible_cell_with_art_becomes_exactly_one_quad() {
        let map = hillside();
        let camera = camera();
        let quads = collect(
            &map,
            &camera,
            &grass_atlas(),
            &TexmapAtlas::pack([]).unwrap(),
            &Cutaway::OPEN,
        );

        let (xs, ys) = camera
            .visible_tiles()
            .clamp_to(map.width(), map.height())
            .expect("the camera is looking at the map");
        let mut expected: Vec<(i32, i32)> = Vec::new();
        let mut dropped = 0;
        for y in ys {
            for x in xs.clone() {
                if map.land(x, y).unwrap().tile == openshard_tiles::LandTileId(MISSING.0) {
                    dropped += 1;
                    continue;
                }
                let at = camera.to_screen(Point::new(x, y, 0));
                expected.push((at.x, at.y));
            }
        }

        // Both halves of the fixture have to be real: a map with nothing missing
        // would leave the drop untested, and one with nothing drawn would make
        // the comparison below a comparison of two empty lists.
        assert!(dropped > 50, "only {dropped} tiles had no art");
        assert!(expected.len() > 1000, "only {} tiles were drawn", expected.len());

        // A tile's screen position identifies it: `(x - y, x + y)` recovers both
        // coordinates, so two tiles cannot share one.
        let mut got: Vec<(i32, i32)> = quads.iter().map(|quad| (quad.x as i32, quad.y as i32)).collect();
        got.sort_unstable();
        expected.sort_unstable();
        assert_eq!(got.len(), quads.len(), "a tile was drawn twice");
        assert_eq!(got, expected);
    }

    /// A quad goes where the camera says its tile goes, height and all ignored.
    ///
    /// Tied to the projection's own numbers — half a tile on each axis — rather
    /// than to `to_screen`, so this and `camera.rs` agreeing is evidence rather
    /// than one formula restated.
    #[test]
    fn a_tile_is_drawn_where_the_camera_puts_it() {
        let map = hillside();
        let camera = camera();
        let quads = collect(
            &map,
            &camera,
            &grass_atlas(),
            &TexmapAtlas::pack([]).unwrap(),
            &Cutaway::OPEN,
        );
        let at = |point: Point| {
            let screen = camera.to_screen(point);
            quads
                .iter()
                .find(|quad| (quad.x as i32, quad.y as i32) == (screen.x, screen.y))
                .copied()
        };

        // The camera's own tile sits in the middle of the viewport.
        let center = at(Point::new(32, 32, 0)).expect("the tile under the camera is drawn");
        assert_eq!((center.x, center.y), (100.0, 80.0));

        // And a step east is half a tile right and half a tile down.
        let east = at(Point::new(33, 32, 0)).expect("the tile east of the camera is drawn");
        assert_eq!((east.x, east.y), (122.0, 102.0));
        let south = at(Point::new(32, 33, 0)).expect("the tile south of the camera is drawn");
        assert_eq!((south.x, south.y), (78.0, 102.0));
    }

    /// Height moves a quad's corners and never the quad.
    ///
    /// The shader lifts each corner by its own height; folding a representative
    /// one into the position here would count it twice, and the picture would
    /// only look subtly wrong on a slope. Two maps of identical shape and
    /// different heights must therefore draw the same tiles in the same places,
    /// and only their corners may differ.
    ///
    /// Compared by position and not pairwise down the two lists: height also
    /// decides how tiles at one screen depth sort against each other, so the two
    /// frames are legitimately in different *orders*. That is the sort's business
    /// and the sibling test's subject.
    #[test]
    fn height_moves_the_corners_and_not_the_quad() {
        let camera = camera();
        let texmaps = TexmapAtlas::pack([]).unwrap();
        let atlas = grass_atlas();

        let hilly = hillside();
        let flat = WorldMap::from_blocks(BlockExtent { wide: 8, down: 8 }, |x, y| LandCell {
            tile: hilly.land(x, y).unwrap().tile,
            z: 0,
        });
        let corners_by_position = |map: &WorldMap| -> std::collections::BTreeMap<(i32, i32), [i32; 4]> {
            collect(map, &camera, &atlas, &texmaps, &Cutaway::OPEN)
                .iter()
                .map(|quad| ((quad.x as i32, quad.y as i32), quad.corners.map(|z| z as i32)))
                .collect()
        };
        let hilly = corners_by_position(&hilly);
        let level = corners_by_position(&flat);

        assert!(hilly.len() > 1000, "only {} quads", hilly.len());
        assert_eq!(
            hilly.keys().collect::<Vec<_>>(),
            level.keys().collect::<Vec<_>>(),
            "a tile moved when its height changed"
        );
        assert_eq!(level.values().filter(|corners| **corners != [0; 4]).count(), 0);
        let differing = hilly
            .iter()
            .filter(|(at, corners)| level[at] != **corners)
            .count();
        assert!(differing > 1000, "only {differing} quads changed shape");
    }

    /// Quads come back far to near, which is what makes the depth each one
    /// carries agree with the order they are drawn in.
    ///
    /// Read off the screen position rather than the sort key: a tile further down
    /// the screen is nearer, and `y` is `(x + y) * 22` at height zero, so a
    /// non-decreasing `y` *is* the back-to-front claim. The depth runs the other
    /// way because nearer is smaller — see [`crate::depth`].
    #[test]
    fn quads_come_back_far_to_near_and_their_depths_agree() {
        let quads = collect(
            &hillside(),
            &camera(),
            &grass_atlas(),
            &TexmapAtlas::pack([]).unwrap(),
            &Cutaway::OPEN,
        );
        assert!(quads.len() > 1000);

        for pair in quads.windows(2) {
            let [near, far] = [&pair[0], &pair[1]];
            assert!(near.y <= far.y, "{} is drawn before {}", near.y, far.y);
            if near.y < far.y {
                assert!(
                    near.depth > far.depth,
                    "a nearer quad has to carry a smaller depth"
                );
            }
        }
    }

    /// A camera looking off the edge of the facet draws nothing, and does not
    /// index anything on the way to finding that out.
    #[test]
    fn a_camera_past_the_edge_of_the_map_draws_nothing() {
        let map = hillside();
        // The facet is 64 tiles square; this looks at a corner of the world far
        // beyond it, which `clamp_to` answers with `None`.
        let camera = Camera::new(Point::new(4000, 4000, 0), 200, 160);
        let quads = collect(
            &map,
            &camera,
            &grass_atlas(),
            &TexmapAtlas::pack([]).unwrap(),
            &Cutaway::OPEN,
        );
        assert!(quads.is_empty(), "{} quads off the map", quads.len());
        assert!(visible_graphics(&map, &camera).is_empty());
    }

    /// What the atlas is asked to hold is what the *map* names, not what it can
    /// already draw — otherwise the atlas would only ever contain what it
    /// contains, and a tile new to the frame would never be packed.
    #[test]
    fn the_wanted_graphics_are_the_maps_and_not_the_atlass() {
        let map = hillside();
        let wanted = visible_graphics(&map, &camera());
        assert_eq!(
            wanted.iter().copied().collect::<Vec<_>>(),
            vec![GRASS, MISSING],
            "the graphic with no art is still asked for"
        );
    }

    /// The instance buffer's layout is a contract with the shader, and the
    /// shader is text the compiler here never sees. If one side changes, this
    /// is the only thing that notices.
    #[test]
    fn a_quad_writes_its_stride_and_nothing_more() {
        let quad = GroundQuad {
            x: 1.0,
            y: 2.0,
            corners: [3.0, 4.0, 5.0, 6.0],
            region: Region {
                u: 0.25,
                v: 0.5,
                du: 0.125,
                dv: 0.125,
            },
            depth: 0.5,
            texmap: Some(Region {
                u: 0.75,
                v: 0.5,
                du: 0.03125,
                dv: 0.03125,
            }),
            place: crate::place::Place::land(7, 9),
        };
        let mut out = Vec::new();
        quad.write(&mut out);
        assert_eq!(out.len() as u64, GroundQuad::STRIDE);
        assert_eq!(&out[..4], &1.0f32.to_le_bytes());
        // The corner heights sit between the position and the region, and the
        // shader reads them as one `vec4` at offset 8.
        assert_eq!(&out[8..12], &3.0f32.to_le_bytes());
        assert_eq!(&out[20..24], &6.0f32.to_le_bytes());
        assert_eq!(&out[24..28], &0.25f32.to_le_bytes());
        // And the texture region is the last four, at offset 40.
        assert_eq!(&out[40..44], &0.75f32.to_le_bytes());
        assert_eq!(&out[52..56], &0.03125f32.to_le_bytes());
        // The tile follows the depth, as two words rather than floats.
        assert_eq!(&out[60..64], &(7u32 | 9 << 16).to_le_bytes(), "the tile");
    }

    /// A tile with no texture still writes a full instance — the buffer is a
    /// stride, not a list — and says so with a zero *size* rather than a zero
    /// position, which is a real corner of the atlas.
    #[test]
    fn a_quad_with_no_texture_writes_a_region_of_no_size() {
        let quad = GroundQuad {
            x: 0.0,
            y: 0.0,
            corners: [0.0; 4],
            region: Region {
                u: 0.0,
                v: 0.0,
                du: 0.021484375,
                dv: 0.021484375,
            },
            texmap: None,
            depth: 0.25,
            place: crate::place::Place::land(0, 0),
        };
        let mut out = Vec::new();
        quad.write(&mut out);
        assert_eq!(out.len() as u64, GroundQuad::STRIDE);
        assert_eq!(&out[48..52], &0.0f32.to_le_bytes(), "du");
        assert_eq!(&out[52..56], &0.0f32.to_le_bytes(), "dv");
        // And the depth is the last float, after the texture region.
        assert_eq!(&out[56..60], &0.25f32.to_le_bytes(), "depth");
    }

    #[test]
    fn a_quad_distinguishes_a_self_contained_flat_tile_from_a_shared_slope() {
        let mut quad = GroundQuad {
            x: 0.0,
            y: 0.0,
            corners: [7.0; 4],
            region: Region {
                u: 0.0,
                v: 0.0,
                du: 1.0,
                dv: 1.0,
            },
            texmap: None,
            depth: 0.0,
            place: crate::place::Place::land(0, 0),
        };
        assert!(quad.is_flat());
        quad.corners[3] = 8.0;
        assert!(!quad.is_flat());
    }

    /// The whole point of corner heights: a corner belongs to four tiles at
    /// once, and all four have to name the same number for it.
    ///
    /// Stated through [`corner_heights`] itself rather than against `map.land`,
    /// so it is a claim about the relation between neighbours and not a second
    /// copy of the lookup. Coverage measured at one camera would stay green
    /// through a swapped pair here; this would not.
    #[test]
    fn neighbours_agree_on_the_corner_they_share() {
        // A patch where heights actually differ: a level one would satisfy any
        // permutation of the four. This ran against Britain's hillside in a real
        // install until `WorldMap::from_blocks` existed, and the fixture is rough for
        // the same reason that patch was chosen.
        let map = hillside();

        let mut differing = 0;
        for y in 0..63u16 {
            for x in 0..63u16 {
                let own = map.land(x, y).expect("inside the facet").z;
                let corners = corner_heights(&map, x, y, own);
                assert_eq!(corners[0], f32::from(own), "({x}, {y}) lost its own height");
                for (index, (dx, dy)) in [(1, 0), (0, 1), (1, 1)].into_iter().enumerate() {
                    let (nx, ny) = (x + dx, y + dy);
                    let neighbour = map.land(nx, ny).expect("inside the facet").z;
                    assert_eq!(
                        corners[index + 1],
                        f32::from(neighbour),
                        "({x}, {y}) and ({nx}, {ny}) disagree about the corner they share",
                    );
                }
                if corners.iter().any(|z| *z != corners[0]) {
                    differing += 1;
                }
            }
        }
        assert!(differing > 100, "only {differing} tiles in the patch slope");
    }

    /// At the edge of the world a tile stands in for the neighbour it does not
    /// have, which flattens the border rather than dropping it into nothing.
    ///
    /// Only ever reachable at a facet's own boundary, which is why it was never
    /// asserted: the fixture is 64 tiles square and its corner is two lines away.
    #[test]
    fn the_last_tile_borrows_its_own_height_for_the_corners_it_has_no_neighbour_for() {
        let map = hillside();
        let corner = map.land(63, 63).expect("the far corner of the fixture");
        assert_eq!(corner_heights(&map, 63, 63, corner.z), [f32::from(corner.z); 4]);

        // And one tile in from the edge borrows only the half it is missing.
        let east = map.land(63, 30).expect("on the east edge");
        let south = map.land(63, 31).expect("its southern neighbour");
        assert_eq!(
            corner_heights(&map, 63, 30, east.z),
            [
                f32::from(east.z),
                f32::from(east.z),
                f32::from(south.z),
                f32::from(east.z)
            ],
            "the two corners past the edge stand in; the one south of it does not"
        );
    }

    /// A window with no map under it at all, which is the row walk's other end.
    ///
    /// The rectangle ends before the map begins, so every row is padding: the
    /// leading fallback already fills it, and the walk is skipped rather than
    /// run from the map's own first column for cells the padding would drop.
    /// The answer is the same either way — this is what says so, and what
    /// would catch a later rewrite that filled the row from the wrong end.
    #[test]
    fn a_window_that_ends_before_the_map_begins_is_all_fallback() {
        let map = hillside();
        // `gather` widens by one to reach the fringe, so `max_x = -2` is the
        // last rectangle whose every column is still west of the map.
        for (min_x, min_y, max_x, max_y) in [(-9, 3, -2, 9), (3, -9, 9, -2), (-9, -9, -2, -2)] {
            let window = LandWindow::gather(&map, min_x, min_y, max_x, max_y);
            for y in min_y..=max_y {
                for x in min_x..=max_x {
                    assert_eq!(window.at(x, y), None, "({x}, {y}) of a window off the map");
                }
            }
        }
    }

    /// [`LandWindow`] is a copy made for speed, so the only thing that makes it
    /// safe is that it answers what the map answers — everywhere, including the
    /// places where "what the map answers" is a fallback rather than a cell.
    ///
    /// The rectangle deliberately starts off the map's north-west corner and
    /// ends past its south-east one: negative coordinates and a fringe with no
    /// cell behind it are exactly where a window built out of casts and
    /// unchecked indices would quietly answer with somebody else's tile.
    #[test]
    fn the_land_window_answers_what_the_map_answers() {
        let map = hillside();
        let (min_x, min_y, max_x, max_y) = (-3, -2, 66, 65);
        let window = LandWindow::gather(&map, min_x, min_y, max_x, max_y);

        let mut cells = 0;
        for y in min_y..=max_y {
            for x in min_x..=max_x {
                let expected = match (u16::try_from(x), u16::try_from(y)) {
                    (Ok(x), Ok(y)) => map.land(x, y),
                    _ => None,
                };
                assert_eq!(window.at(x, y), expected, "the cell at ({x}, {y})");
                let Some(cell) = expected else {
                    continue;
                };
                cells += 1;
                assert_eq!(
                    window.corners(x, y, cell.z),
                    corner_heights(&map, x as u16, y as u16, cell.z),
                    "the corners at ({x}, {y})",
                );
            }
        }
        assert_eq!(cells, 64 * 64, "the fixture's every tile was covered");
    }
}
