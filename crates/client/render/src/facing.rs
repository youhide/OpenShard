//! Which edge of its tile a wall stands on, measured from the wall's own art.
//!
//! Nothing in `tiledata.mul` records this. `docs/lighting.md`'s decision 3 says
//! so and is right: there is no flag, no byte and no table for which of a tile's
//! four edges a `WALL` static occupies. The *picture* knows, because in this
//! projection a tile edge is half a cell wide with a 45° run, and a wall drawn on
//! one edge cannot be drawn on any of the other three without looking different.
//!
//! # What is measured
//!
//! The **base edge**: the lowest drawn pixel of each column of the sprite. That
//! is the line where the wall meets the ground, and it is the one feature of a
//! wall's silhouette with no ornament on it — the top of a wall carries
//! crenellations, eaves and antialiased tips, and the sides are cut by whatever
//! the artist drew standing against them. Two numbers come out of it:
//!
//! - **Which half of the tile's column the wall occupies.** A tile's diamond
//!   spans 22 pixels either side of the column the sprite is centred on; each of
//!   the four edges covers exactly one of those halves. North and east are the
//!   right half, south and west the left.
//! - **Which way the base edge runs.** It descends to the right for north and
//!   south, and to the left for east and west, because those are the two world
//!   axes and this projection turns them into the two screen diagonals.
//!
//! Those two bits are the four faces, and they are independent, which is what
//! makes the pair a measurement rather than a guess.
//!
//! # A corner is two faces, and it is answered rather than refused
//!
//! Both halves of a corner's column are full, because a corner *is* both faces
//! at once. The first version of this read that as a contradiction and refused
//! it, which left every corner of every building in the world an
//! [`Upright`](crate::place::Stance::Upright) whole-tile occluder — a flat
//! 44-pixel band between two continuous runs of wall, lit on the side turned
//! away from the flame, and leaking a diagonal sliver of light into the room
//! behind it. See `docs/lighting.md`'s decision 25.
//!
//! So the halves are read **twice**. First strictly, each one having to be the
//! only face in the picture: that is the whole of what this module did before
//! corners, and a graphic it reads keeps exactly the answer it had. Only when
//! neither half can carry the picture alone are the two offered the picture
//! together, and a corner is two independent measurements that each passed every
//! gate but the one about the other half.
//!
//! # And what it still refuses
//!
//! A detector that cannot say "I do not know" is the failure mode here: a wrong
//! face is a wall shaded along an axis it does not run on, and every graphic
//! this is offered is a graphic somebody's shard draws. The gates, each of which
//! a real client graphic fails:
//!
//! - A **post** (`0x0101`) covers neither half: its base is a few columns wide
//!   with a level bottom, so no run of 45° can be fitted to it. Neither half
//!   reads, so there is no corner either — two failures are not a corner, and
//!   that is the property the second pass rests on.
//! - Anything whose base is not straight — a tree, a barrel, a fence with a gap
//!   — fails the straightness test over the half it claims.
//!
//! Undecided is not a defect and costs nothing: [`crate::place::Stance`] falls
//! back to `Upright`, which is what every static did before this module existed.
//!
//! # Where the numbers came from
//!
//! Read off the client's own art rather than derived: `0x0100` "marble wall" has
//! its mass in columns 18..=43 of a 44-wide sprite with the base descending to
//! the left, which is the east face, and its base line lands on the predicted
//! `dy = 22 - across` to the pixel over the whole 22-column span. `0x0007` is
//! the south face of the same shape, and `0x0104` is the corner, which is both.
//! The sweep in `tests/facing.rs` is what says how much of a real install this
//! reads, because a detector with no coverage count is a green light for having
//! checked nothing.
//!
//! # And the hole in it
//!
//! [`aperture_of`] is the second measurement off the same silhouette — step 16 of
//! `docs/lighting.md`. A window is a *hole in a wall*, so what the art left
//! transparent inside an opaque face is the rectangle a ray passes through. It
//! lives beside the face rather than in a module of its own because it needs one:
//! which half of the picture is a surface, and which way the run counts along it,
//! are the face's answers, and the two verdicts are one row of one table.

use openshard_uofiles::image::Image;

/// Which version of the rules below a measurement was made by.
///
/// It rides in an [`ArtTable`](crate::arttable::ArtTable)'s stamp, and what it
/// buys is the staleness this file's numbers would otherwise be invisible in: a
/// table written when `SPILL` was six describes a detector that read 40% of a
/// city, and it looks exactly as fresh as one written today. The art it was
/// measured from has not changed, so nothing else in the stamp can say so.
///
/// **Bump it when a gate here changes** — `MIN_FILLED`, `SPILL`, `OVERHANG`,
/// `STRAIGHT`, `SQUARE`, `OFF_EDGE`, `MIN_STANDING`, `HOLE_MIN_RUN`,
/// `HOLE_MIN_RISE`, `HOLE_MARGIN`, or the shape of [`facing_of`] or
/// [`aperture_of`]. Nothing enforces that, and nothing can: it is a claim about a
/// diff. What catches a bump that was forgotten is the sweep in
/// `openshard-client-artscan`, which reads a real install and compares every row
/// of a table against a live measurement — see that crate's `agrees` test.
///
/// **Two** since the hole joined the face: a table written by detector 1 has a
/// row for every window and a hole in none of them, and nothing else in the
/// stamp could say so.
pub const DETECTOR: u32 = 2;

/// Which edge of its tile a wall stands on.
///
/// Named for the world direction the edge faces *out* of the tile, which is the
/// same naming the map uses: the north edge is the one at `y` = the tile's own,
/// and a wall on it runs along `+x`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Face {
    /// The `y0` edge, running along `+x`. The upper-right side of the diamond.
    North,
    /// The `x1` edge, running along `+y`. The lower-right side.
    East,
    /// The `y1` edge, running along `+x`. The lower-left side.
    South,
    /// The `x0` edge, running along `+y`. The upper-left side.
    West,
}

impl Face {
    /// Where in its tile a point `run` of the way along this face is, as the
    /// fraction pair the place attachment carries.
    ///
    /// `run` is `0` at the edge's start and `1` at its end, following the world
    /// axis the edge lies along — so the *next* tile's face starts its run at 0
    /// where this one ended at 1, and the two name one world line. That is the
    /// whole point of measuring the face at all: a row of wall tiles stops being
    /// a row of sprites and becomes one continuous surface.
    ///
    /// The Rust copy of what `statics.wgsl` does per fragment, and it exists so
    /// the seam property can be stated in a unit test rather than only in a
    /// rendered frame.
    pub fn place_at(self, run: f32) -> (f32, f32) {
        let run = run.clamp(0.0, 1.0);
        match self {
            Self::North => (run, 0.0),
            Self::East => (1.0, run),
            Self::South => (run, 1.0),
            Self::West => (0.0, run),
        }
    }

    /// Which way this face looks, in tiles.
    ///
    /// The *drawn* direction, which is the same thing: the art only ever draws
    /// the two faces an isometric camera can see, so a south face's picture is
    /// the surface turned towards `+y` and an east face's towards `+x`. North and
    /// west are five graphics out of 1197 and exist here because the geometry has
    /// four edges.
    ///
    /// What it is for is the lighting: a wall's two faces are one tile, one
    /// plane, one fraction and one height, so nothing else in the attachment can
    /// tell the street side of a house from the room side. `blit.wgsl`'s
    /// `outward`, and the two are one table.
    pub fn outward(self) -> [f32; 2] {
        match self {
            Self::North => [0.0, -1.0],
            Self::East => [1.0, 0.0],
            Self::South => [0.0, 1.0],
            Self::West => [-1.0, 0.0],
        }
    }

    /// Whether the run of wall this face belongs to lies along `+x`.
    ///
    /// A north or south face stands on a `y` edge, so its run is a row; an east
    /// or west face stands on an `x` edge, so its run is a column. What asks is
    /// anything that treats a run of wall as one surface — see
    /// [`light::own_run`](crate::light) and the facing test beside it.
    pub fn runs_along_x(self) -> bool {
        matches!(self, Self::North | Self::South)
    }

    /// Where this face's own edge is on the screen: how far *below* the tile's
    /// centre row the ground line is, for a pixel `across` pixels from the tile's
    /// column.
    ///
    /// The forward projection of one edge of the diamond, and the thing a wall's
    /// base line has to land on. Unlike [`Face::run_at`] it does not saturate —
    /// it is a line, and asking where it is outside the face's own half is a
    /// question with an answer.
    pub fn edge_at(self, across: f32) -> f32 {
        match self {
            Self::North => across - HALF_TILE_WIDTH,
            Self::East => HALF_TILE_WIDTH - across,
            Self::South => across + HALF_TILE_WIDTH,
            Self::West => -across - HALF_TILE_WIDTH,
        }
    }

    /// How far along this face a pixel `across` pixels from the tile's own
    /// column is, as a `0..=1` run. The inverse of the projection, for one edge.
    ///
    /// Outside the face's own half this saturates rather than extrapolating: a
    /// wall sprite carries a few pixels of its own *thickness* past the tile's
    /// centre column, and those pixels belong to the near end of the edge and
    /// not to a place outside the tile.
    pub fn run_at(self, across: f32) -> f32 {
        let v = across / HALF_TILE_WIDTH;
        match self {
            Self::North => v,
            Self::East => 1.0 - v,
            Self::South => 1.0 + v,
            Self::West => -v,
        }
        .clamp(0.0, 1.0)
    }
}

/// A tile's width in the drawn image, and half of it — which is also how wide
/// one edge of the diamond is, since each edge spans one half of the column.
///
/// `crate::camera::TILE_WIDTH` is the same 44 and this module cannot borrow it
/// without pulling a camera into a function that is handed nothing but pixels.
/// Pinned against it in the tests below.
const TILE_WIDTH: f32 = 44.0;
const HALF_TILE_WIDTH: f32 = TILE_WIDTH / 2.0;

/// How many of a face's 22 columns must actually be drawn for it to be a face.
///
/// Not all 22: the far end of an edge tapers to a point, and the last column or
/// two of a real sprite is antialiased away to nothing.
const MIN_FILLED: usize = 18;

/// How far past the tile's centre column the *other* half may be drawn on.
///
/// A wall is a solid with a thickness, and the picture shows that thickness: the
/// far side of the face is a sliver past the edge (3.5 pixels on `0x0100`, 2.5 on
/// `0x0007`), and where the wall is low enough to look down on, its whole *top*
/// surface is drawn as well — 8.5 pixels on `0x0063`, the low garden wall Britain
/// is fenced with. A thickness of `t` tiles projects to `22t` pixels across, so
/// twelve is a wall half a tile thick, which is thicker than any the client
/// ships.
///
/// The number that matters is the gap to the thing this has to tell it apart
/// from: a corner is two faces and covers the *whole* other half, 21.5 pixels of
/// it. Twelve sits between the two with room on both sides, and it was chosen by
/// measuring both — at six, 40% of the walls standing in Britain were read; at
/// twelve, 76%. See `tests/facing.rs`.
///
/// It is still the line between one face and two now that both are answers: what
/// it decides is whether a picture is a wall with its thickness showing or a
/// corner with a second surface on it, and those are shaded and occluded
/// differently. A thickness read as a face would give the tile a panel on an edge
/// nothing stands on, which stops a ray that should pass.
const SPILL: f32 = 12.0;

/// How far past the tile's own column anything may be drawn at all.
///
/// Two pixels, which is an antialiased edge. Beyond it the picture is of
/// something bigger than one cell — a whole building, a multi-tile tree — and
/// none of the four faces is an answer about it.
const OVERHANG: f32 = 2.0;

/// How far a base pixel may sit off the 45° line fitted through the ends.
///
/// One pixel is antialiasing and rounding; three is a different shape.
const STRAIGHT: i32 = 2;

/// How far the run of the fitted line may be from its rise, in pixels. A wall's
/// base is at 45° exactly — that is the projection, not a style — so this is
/// tolerance for the ends being blunt rather than for a slope being different.
const SQUARE: i32 = 3;

/// How far a base pixel may sit from the tile edge the face names, in pixels.
///
/// Three: the antialiasing, the half-pixel of an odd-width sprite, and a pixel
/// of headroom over the widest real one. Measured — over the wall graphics this
/// reads, the median distance is exactly zero and the largest is two — and the
/// headroom is there because a tolerance sitting exactly on the widest thing it
/// has seen is a tolerance that has not been tested.
///
/// This is a *position* and not a slope, which is what makes it the one gate
/// here with nothing to argue about: see where it is used.
const OFF_EDGE: f32 = 3.0;

/// How many pixels up the screen one unit of `z` is.
///
/// `crate::camera::Z_STEP` is the same four, and this module cannot borrow it for
/// the reason [`TILE_WIDTH`] is spelled out here: a function handed nothing but
/// pixels should not have to be handed a camera. Pinned against it in the tests
/// below.
const Z_STEP: f32 = 4.0;

/// How many columns of a face a hole must span before it is a hole.
///
/// Three, which is a hole a fifth of a tile wide at its narrowest. Under it are
/// the two things a wall's picture has that are not windows: the single stray
/// transparent pixel an artist left inside a run, and the one-column notch
/// between two bricks. Both would otherwise be measured as a slot for light to
/// come through — and unlike a refused face, a wrong hole is *brighter* than the
/// truth, which is the direction this pass refuses everywhere else.
const HOLE_MIN_RUN: usize = 3;

/// And how tall it must stand, in pixels.
///
/// Eight, which is two units of `z` — the same order as the width gate, and for
/// the same reason. A hole one `z` tall is a scratch in the art.
const HOLE_MIN_RISE: i32 = 8;

/// How many solid columns of the face must stand either side of the hole.
///
/// **The gate that says a hole is a hole and not an edge.** A window is
/// surrounded by its wall; a gap that runs to the end of the picture is the space
/// between two things the artist drew in one sprite — an arch's leg, a fence's
/// post, a wall with a pillar beside it — and reading it as an aperture would cut
/// a hole through a surface whose *silhouette* stops there anyway.
///
/// Two rather than one: the last drawn column of a face is antialiasing, so a
/// one-column margin is a margin that may not be picture at all.
const HOLE_MARGIN: usize = 2;

/// How tall the wall must stand over its base, in pixels, before this is willing
/// to call it a wall.
///
/// Four units of `z`. Under it are the slabs — a roof piece, a step, a low
/// railing — whose base can be a clean 45° run without the thing being a
/// billboard whose picture is height.
const MIN_STANDING: u16 = 16;

/// What the art says a static's picture is a surface of: one face, or the two of
/// a corner.
///
/// The corner's two are always **one from each half** of the tile's column, which
/// is not a convention but the way the halves are read: the right half can only
/// answer `North` or `East` and the left half only `South` or `West`. So `right`
/// and `left` are the two questions and neither can hold the other's answer.
///
/// Two faces and not four: nothing in a picture 44 pixels wide can be a third
/// surface, because a tile's column has two halves and each of them is one edge.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Facing {
    /// A plain wall: one edge of the tile, and nothing on the other half of the
    /// picture but the wall's own thickness.
    One(Face),
    /// A corner: both halves of the column are a face, each measured on its own.
    Corner {
        /// The face on the right half of the tile's column — `North` or `East`.
        right: Face,
        /// The face on the left half — `South` or `West`.
        left: Face,
    },
}

impl Facing {
    /// Which of the faces a pixel `across` pixels from the tile's own column
    /// belongs to.
    ///
    /// The rule `statics.wgsl` applies per fragment, and it is the whole of how a
    /// corner is resolved: a corner's two faces are two halves of one picture, so
    /// the half a pixel is drawn on names which surface it is a pixel of. A
    /// pixel exactly on the middle column goes to the left face, which is the
    /// half the sign convention puts zero in — it is one column of a 44-wide
    /// sprite and either answer is a face of the same corner.
    ///
    /// The Rust copy of that switch, for [`crate::plan`]'s elevation and for
    /// anything on the CPU that has to say what the attachment will hold.
    pub fn on_half(self, across: f32) -> Face {
        match self {
            Self::One(face) => face,
            Self::Corner { right, left } => match across > 0.0 {
                true => right,
                false => left,
            },
        }
    }

    /// Every face this picture is a surface of, in the order the halves are read.
    ///
    /// What asks is anything that has to answer about the *tile* rather than
    /// about a pixel of it — [`edges_of`](crate::occlusion::edges_of), which
    /// turns them into the sides of the cell a ray can be stopped by.
    pub fn faces(self) -> impl Iterator<Item = Face> {
        let (first, second) = match self {
            Self::One(face) => (face, None),
            Self::Corner { right, left } => (right, Some(left)),
        };
        std::iter::once(first).chain(second)
    }
}

/// Which edges of its tile this static stands on, or `None` if the art does not
/// say.
///
/// Pure: an image in, a verdict out, no files and no state. Called once per
/// graphic while the atlas packs it — see
/// [`StaticAtlas::insert`](crate::atlas::StaticAtlas) — because the answer is a
/// property of the picture and a picture is packed once.
///
/// The cost is one pass over the sprite's pixels, which is the pass that
/// [`copy_sprite`](crate::atlas) is making anyway. The halves are read at most
/// twice over that one pass; nothing looks at the image again.
pub fn facing_of(image: &Image) -> Option<Facing> {
    let width = image.width();
    // Narrower than a tile cannot hold a whole edge in the half it belongs to:
    // the sprite is centred on the tile's column, so a 22-wide picture reaches
    // 11 pixels either side and covers no edge at all.
    if f32::from(width) < TILE_WIDTH {
        return None;
    }
    let base = base_edge(image);
    let height = image.height();
    // Strictly first: each half proposed as the *only* face in the picture, the
    // other half allowed nothing past a wall's own thickness. A graphic that
    // reads here reads exactly what it read before corners existed, which is
    // what keeps this change from moving three quarters of a city's walls.
    for half in [Half::Right, Half::Left] {
        if let Some(face) = half.read(&base, width, height, Second::Refused) {
            return Some(Facing::One(face));
        }
    }
    // And then together. Both halves have already failed alone, so the only way
    // through here is that each of them is a face and the other one is why it
    // was refused — which is what a corner is. A picture where one half is a
    // face and the other is a blob still fails, because the blob is not a face.
    let right = Half::Right.read(&base, width, height, Second::Allowed)?;
    let left = Half::Left.read(&base, width, height, Second::Allowed)?;
    Some(Facing::Corner { right, left })
}

/// A hole in a wall, measured off the wall's own picture, in the surface's own
/// coordinates.
///
/// What [`aperture_of`] answers and what a row of an
/// [`ArtTable`](crate::arttable::ArtTable) carries. It is deliberately *not*
/// [`Aperture`](crate::occlusion::Aperture), which is the same rectangle placed
/// in the world: a picture is drawn once and stood on a hundred tiles at a
/// hundred heights, so the measurement can only be relative to the thing it was
/// measured from. [`Aperture::above`](crate::occlusion::Aperture::above) is where
/// the two meet, and it is called once per static with that static's own `z`.
///
/// - `near` and `far` run along the face, in
///   [`RUN_STEPS`](crate::occlusion::RUN_STEPS)ths of a tile, counted the way
///   [`Face::run_at`] counts — so `near` is the low corner of the world axis the
///   face lies along whichever way the picture happens to be drawn.
/// - `bottom` and `top` are `z` **above the static's own base**, in the map's
///   units.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Hole {
    /// Where the hole starts along the run.
    pub near: u8,
    /// And where it ends.
    pub far: u8,
    /// The lowest `z` it reaches, above the static's base.
    pub bottom: u8,
    /// And the highest.
    pub top: u8,
}

/// The hole in a wall's face, or `None` where the art draws none.
///
/// Pure, like [`facing_of`], and called next to it: `facing` is the verdict that
/// function already reached, passed in rather than measured again, because a
/// hole is a rectangle *in a face* and there is nothing to measure without one.
///
/// # What is measured
///
/// A face is a plane, and this projection draws it so that a point on it has a
/// screen column that depends only on how far along the run it is, and a screen
/// row that is its base line minus its height. So a rectangle in the surface's
/// coordinates is drawn as a parallelogram: **vertical sides at the two ends of
/// the run, and 45° top and bottom that descend with the base line**. Which means
/// the whole measurement is per column: the transparent run inside a column's own
/// picture, taken as a height above that column's base pixel, is the hole's `z`
/// span there — and it is the same span in every column the hole covers.
///
/// # The rectangle is the largest one that fits
///
/// The client's windows are not rectangles. `0x003C` — the one every third house
/// in Britain has — is an arch: a doorway with a flat sill, straight sides and a
/// rounded top, and its transparent region is two pixels taller in the middle
/// than at the ends. There is one honest rectangle in a shape like that and it is
/// the **largest one inscribed in it**: a bounding box would let light through
/// stone the artist drew.
///
/// So the columns' spans are searched for the sub-run of greatest area, which is
/// `O(n²)` over at most 22 columns — the sort of arithmetic decision 31's budget
/// was bought for, and it would have been a scanline trick in a frame.
///
/// # And what it refuses
///
/// The same refusal culture as the face above it, with one extra reason to be
/// careful: a face this cannot read is a wall shaded oddly, and a *hole* this
/// reads wrongly is light coming through a stone wall. So:
///
/// - **A corner.** Two faces, one picture, and a hole measured on one half of it
///   would be given to both panels — a window in the wall it is cut into and a
///   window in the wall beside it. Nothing in a silhouette says which half a hole
///   belongs to, so this says nothing.
/// - **A gap that reaches the end of the face** — [`HOLE_MARGIN`]. That is the
///   space between two things drawn in one picture, not a hole through one thing.
/// - **Two gaps in one column**, and **gaps that do not stand in one run of
///   columns**: either is two holes, and a surface carries one.
/// - **Anything smaller than [`HOLE_MIN_RUN`] by [`HOLE_MIN_RISE`]**, which is
///   the stray transparent pixel and the notch between two bricks.
pub fn aperture_of(image: &Image, facing: Facing) -> Option<Hole> {
    // A corner is refused before anything is measured — see above.
    let Facing::One(face) = facing else {
        return None;
    };
    let width = image.width();
    let middle = f32::from(width) / 2.0;
    let base = base_edge(image);

    // The face's own half, in column order, with each column's single gap. The
    // *other* half is the wall's thickness (`SPILL`) and whatever the artist drew
    // standing against it, and neither is a surface a ray crosses.
    let mut columns: Vec<Column> = Vec::new();
    for (column, bottom) in base.columns() {
        let across = f32::from(column) + 0.5 - middle;
        let into = match face {
            Face::North | Face::East => across,
            Face::South | Face::West => -across,
        };
        if into <= 0.0 || into > HALF_TILE_WIDTH {
            continue;
        }
        // Every column of the half has a top, since it has a bottom.
        let top = base.top[usize::from(column)].unwrap();
        let gap = match gap_in(image, column, top, bottom) {
            // Two holes in one column is not one rectangle, and the surface
            // carries one. Refused for the whole picture rather than for the
            // column, because "the hole is wherever the picture is simple" is a
            // measurement of the detector rather than of the art.
            Gap::Several => return None,
            Gap::Solid => None,
            // Heights above this column's own base pixel, which is what makes
            // the 45° descent drop out: a hole level in the surface is a
            // constant here and a slanted one is not.
            Gap::One(from, to) => Some((
                i32::from(bottom) - i32::from(to),
                i32::from(bottom) - i32::from(from),
            )),
        };
        columns.push(Column { column, gap });
    }

    // Where the gap columns are, and that they are one run of them with wall
    // either side.
    let first = columns.iter().position(|column| column.gap.is_some())?;
    let last = columns.iter().rposition(|column| column.gap.is_some()).unwrap();
    if columns[first..=last].iter().any(|column| column.gap.is_none()) {
        return None;
    }
    if first < HOLE_MARGIN || last + HOLE_MARGIN >= columns.len() {
        return None;
    }

    // The largest rectangle inscribed in the gap: every sub-run of columns, held
    // to the lowest ceiling and the highest floor in it.
    let spans: Vec<(i32, i32)> = columns[first..=last]
        .iter()
        .map(|column| column.gap.unwrap())
        .collect();
    let mut best: Option<(usize, usize, i32, i32)> = None;
    let mut largest = 0;
    for from in 0..spans.len() {
        let mut bottom = i32::MIN;
        let mut top = i32::MAX;
        for (steps, span) in spans[from..].iter().enumerate() {
            bottom = bottom.max(span.0);
            top = top.min(span.1);
            let rise = top - bottom;
            if rise <= 0 {
                break;
            }
            let area = (steps as i32 + 1) * rise;
            if area > largest {
                largest = area;
                best = Some((from, from + steps, bottom, top));
            }
        }
    }
    let (from, to, bottom, top) = best?;
    if to - from + 1 < HOLE_MIN_RUN || top - bottom < HOLE_MIN_RISE {
        return None;
    }

    // And out into the surface's own coordinates. The run is measured at the
    // *edges* of the end columns rather than at their centres — a hole eight
    // pixels wide is eight pixels of wall missing, and half a pixel at each end
    // of it is the difference between a rectangle and the pixels it was read off.
    let edge = |column: u16, side: f32| face.run_at(f32::from(column) + side - middle);
    let start = edge(columns[first + from].column, 0.0);
    let end = edge(columns[first + to].column, 1.0);
    let step = |run: f32| (run.clamp(0.0, 1.0) * crate::occlusion::RUN_STEPS).round() as u8;
    Some(Hole {
        near: step(start.min(end)),
        far: step(start.max(end)),
        // Nearest rather than inwards: the span is already the inscribed one, and
        // rounding an inscribed rectangle inwards twice is a hole a `z` narrower
        // than the art's on both sides.
        bottom: (bottom as f32 / Z_STEP).round().clamp(0.0, 255.0) as u8,
        top: (top as f32 / Z_STEP).round().clamp(0.0, 255.0) as u8,
    })
}

/// One column of a face: where it is, and the hole in it if it has one, as
/// heights above the column's own base pixel.
struct Column {
    /// Its index in the picture.
    column: u16,
    /// `(bottom, top)` of the transparent run inside it, in pixels above the
    /// base. `None` for a column of solid wall.
    gap: Option<(i32, i32)>,
}

/// What one column has inside its own picture.
enum Gap {
    /// Nothing: every row between its ends is drawn.
    Solid,
    /// One transparent run, `(first row, last row)` inclusive.
    One(u16, u16),
    /// More than one, which is not a rectangle in any plane.
    Several,
}

/// The transparent run strictly inside one column's drawn pixels.
///
/// `top` and `bottom` are that column's own first and last drawn row, so a run
/// found between them has picture above it and picture below it by construction
/// — which is half of what makes a hole a hole rather than a notch. The other
/// half is the margin along the run, and that is [`aperture_of`]'s.
fn gap_in(image: &Image, column: u16, top: u16, bottom: u16) -> Gap {
    let mut found: Option<(u16, u16)> = None;
    let mut open: Option<u16> = None;
    for row in top..=bottom {
        // Inside the rectangle, so there is a pixel; transparency is the
        // question, and it is the client's own rule — see `base_edge`.
        let clear = image.pixel(column, row).unwrap().is_transparent();
        match (clear, open) {
            (true, None) => open = Some(row),
            (false, Some(start)) => {
                if found.is_some() {
                    return Gap::Several;
                }
                found = Some((start, row - 1));
                open = None;
            }
            _ => {}
        }
    }
    // A run cannot still be open: `bottom` is a drawn row by construction.
    match found {
        None => Gap::Solid,
        Some((from, to)) => Gap::One(from, to),
    }
}

/// Whether a half being read may have a second face drawn on the other half of
/// the picture.
///
/// Named rather than a `bool` at the call site because the two passes over the
/// same halves differ in nothing else, and "true" there would say nothing about
/// which of them is which.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Second {
    /// The half must be the only face in the picture: anything past a wall's own
    /// thickness on the other half refuses it. The plain-wall question.
    Refused,
    /// The other half may hold whatever it holds — it is being asked the same
    /// question separately. The corner question.
    Allowed,
}

/// Which half of the tile's column a face occupies.
#[derive(Clone, Copy)]
enum Half {
    /// `across` in `(0, 22]` — the north and east edges.
    Right,
    /// `across` in `[-22, 0)` — the south and west edges.
    Left,
}

impl Half {
    /// Which way `across` counts on this half: right is positive, left negative.
    fn sign(self) -> f32 {
        match self {
            Self::Right => 1.0,
            Self::Left => -1.0,
        }
    }

    /// The face on this half of the column, or `None` if the art is not a wall
    /// standing on it.
    ///
    /// `second` is the one thing that differs between the two passes
    /// [`facing_of`] makes: whether a face on the *other* half refuses this one
    /// or is somebody else's business. Everything else — the straightness, the
    /// slope, the standing height, the position of the base line — is a
    /// measurement of this half alone and is made the same way both times.
    fn read(self, base: &BaseEdge, width: u16, height: u16, second: Second) -> Option<Face> {
        let middle = f32::from(width) / 2.0;
        // The columns of this half, and everything the other half may not hold.
        let mut mine: Vec<(i32, u16)> = Vec::new();
        for (column, bottom) in base.columns() {
            let across = f32::from(column) + 0.5 - middle;
            // Drawn outside the tile's own column altogether. A picture wider
            // than one cell is not one wall standing on one edge of it — the
            // client ships whole buildings and multi-tile trees as single
            // graphics, and the first version of this read a 106-pixel statue as
            // a north face because it only ever looked at the half it had
            // proposed. Whatever is on the *other* side of the sprite has to be
            // looked at too, and that is what this line does.
            if across.abs() > HALF_TILE_WIDTH + OVERHANG {
                return None;
            }
            // How far *into* this half the column is: positive on the half being
            // proposed and negative on the other one. Written as one signed
            // number rather than two mirrored comparisons, so that the two halves
            // cannot drift apart — a tolerance loosened on one side only is
            // exactly the shape of bug this whole module is a defence against.
            let into = across * self.sign();
            if into > 0.0 && into <= HALF_TILE_WIDTH {
                mine.push((i32::from(column), bottom));
                continue;
            }
            // Drawn on the wrong side of the tile's centre. A little is the
            // wall's own thickness showing past the edge it stands on; more is a
            // second face, which is a corner. A column past the *far* vertex is
            // neither — it is the antialiased tip `OVERHANG` allows — and it is
            // left out of the fit rather than counted against it.
            //
            // On the corner pass this is not asked at all: the other half is
            // being read as a face in its own right, and the whole question
            // there is whether *this* half is one too.
            if second == Second::Refused && -into > SPILL {
                return None;
            }
        }
        if mine.len() < MIN_FILLED {
            return None;
        }

        let (first_column, first_bottom) = *mine.first().unwrap();
        let (last_column, last_bottom) = *mine.last().unwrap();
        let run = last_column - first_column;
        let rise = i32::from(last_bottom) - i32::from(first_bottom);
        // At 45°, and steeply enough that the sign means something. A level base
        // has a rise of zero and names no direction at all.
        if (rise.abs() - run).abs() > SQUARE || run < MIN_FILLED as i32 {
            return None;
        }
        let descending_right = rise > 0;
        // Straight, not merely straight at the ends: a chevron has the same two
        // endpoints as the line through them.
        let step = if descending_right { 1 } else { -1 };
        for (column, bottom) in &mine {
            let want = first_bottom as i32 + step * (column - first_column);
            if (i32::from(*bottom) - want).abs() > STRAIGHT {
                return None;
            }
        }
        // And it stands up. A slab whose base happens to be a clean 45° run —
        // a roof piece, a low step — is not a billboard whose picture is height,
        // and shading it as one would be worse than leaving it alone.
        if base.standing(last_column.min(first_column) + run / 2) < MIN_STANDING {
            return None;
        }

        let face = match (self, descending_right) {
            (Self::Right, true) => Face::North,
            (Self::Right, false) => Face::East,
            (Self::Left, true) => Face::South,
            (Self::Left, false) => Face::West,
        };

        // And the base line is *where the edge is*, not merely parallel to it.
        //
        // Everything above measures the line's direction and its straightness,
        // and neither pins down where it sits. Nothing has to: a wall's base is
        // where it meets the ground, `statics::stand_on` puts the sprite's bottom
        // row on the diamond's bottom vertex, so the edge's own screen position
        // is fully determined by the face. There is no freedom left, and the
        // client agrees — over the 943 wall graphics this reads, the median
        // distance from the base line to the predicted edge is exactly zero.
        //
        // What it catches is the thing the slope cannot: a picture with the right
        // *direction* somewhere else in the tile. `0x0171` is a flat diamond
        // drawn eighty pixels above its own tile — a roof or an awning — whose
        // lower-right side is a clean 45° run in the right half with nothing in
        // the left, and which passed every other gate here. Shading a horizontal
        // surface as a vertical face is worse than leaving it alone.
        let bottom_row = f32::from(height) - HALF_TILE_WIDTH;
        for (column, bottom) in &mine {
            let across = *column as f32 + 0.5 - middle;
            let drawn = f32::from(*bottom) + 0.5 - bottom_row;
            if (drawn - face.edge_at(across)).abs() > OFF_EDGE {
                return None;
            }
        }
        Some(face)
    }
}

/// The lowest and highest drawn pixel of every column of a sprite.
///
/// One pass, kept as two rows of `Option` rather than as a list of runs: the
/// reads below are by column index and the sprite is at most a few hundred wide.
struct BaseEdge {
    /// Per column: the last drawn row, or `None` for a column with nothing in it.
    bottom: Vec<Option<u16>>,
    /// Per column: the first drawn row. Only the difference is read — see
    /// [`BaseEdge::standing`].
    top: Vec<Option<u16>>,
}

impl BaseEdge {
    /// Every column that has anything drawn in it, left to right, with the row
    /// its lowest pixel is on.
    fn columns(&self) -> impl Iterator<Item = (u16, u16)> + '_ {
        self.bottom
            .iter()
            .enumerate()
            .filter_map(|(column, bottom)| bottom.map(|row| (column as u16, row)))
    }

    /// How tall the picture is in one column, in pixels — nothing for a column
    /// with nothing in it.
    fn standing(&self, column: i32) -> u16 {
        let Ok(column) = usize::try_from(column) else {
            return 0;
        };
        match (self.top.get(column), self.bottom.get(column)) {
            (Some(Some(top)), Some(Some(bottom))) => bottom - top + 1,
            _ => 0,
        }
    }
}

/// Walk the sprite once and record where each column's picture starts and ends.
///
/// "Drawn" is the same question the fragment shader asks and the same one
/// [`StaticAtlas::opaque_at`](crate::atlas::StaticAtlas::opaque_at) asks: a
/// transparent pixel is absent. That is the client's own rule for static art —
/// `ArtLoader.ReadStaticArt` writes a run's pixel only when it is non-zero — so
/// a hole inside a run and a column no run covered are one thing here.
fn base_edge(image: &Image) -> BaseEdge {
    let width = usize::from(image.width());
    let mut edge = BaseEdge {
        bottom: vec![None; width],
        top: vec![None; width],
    };
    for y in 0..image.height() {
        for x in 0..image.width() {
            // `pixel` is `None` only outside the rectangle, which this loop is
            // not; the transparency is the question being asked.
            if image.pixel(x, y).unwrap().is_transparent() {
                continue;
            }
            let column = usize::from(x);
            edge.top[column].get_or_insert(y);
            edge.bottom[column] = Some(y);
        }
    }
    edge
}

/// A wall's silhouette, drawn the way the projection draws one: a parallelogram
/// standing on one edge of the tile's diamond.
///
/// `face` decides which edge, `height` how far the wall rises above it. The
/// picture is 44 wide, which is what the client ships, and its bottom row is the
/// diamond's bottom vertex — [`statics::stand_on`](crate::statics::stand_on) puts
/// every static's bottom edge there whatever the art holds, so a north face,
/// whose lowest pixel is at the diamond's *right* vertex, genuinely has 22 blank
/// rows under it.
///
/// An ordinary `pub` item rather than a `#[cfg(test)]` one, for the reason
/// [`crate::scene`]'s rooms are: the readers are outside this crate. The GPU
/// frame test needs a picture the atlas will read a known face off, and it needs
/// it to be the *same* picture the unit tests below decide against — a second
/// hand-drawn parallelogram in `tests/frame.rs` would be a second opinion about
/// what a wall looks like, and the day the two drifted the frame test would be
/// asserting about a shape this module never sees.
///
/// No client files, and none needed: the shape is the projection, and the
/// projection is arithmetic.
pub fn silhouette(face: Face, height: u16) -> Image {
    use openshard_uofiles::color::Color16;

    let width = 44u16;
    let rows = height + 45;
    let mut pixels = vec![Color16::TRANSPARENT; usize::from(width) * usize::from(rows)];
    for column in 0..width {
        let across = f32::from(column) + 0.5 - f32::from(width) / 2.0;
        // Only the half this face stands on is drawn — the thickness sliver
        // a real sprite has is the subject of its own test below.
        let into = match face {
            Face::North | Face::East => across,
            Face::South | Face::West => -across,
        };
        if into <= 0.0 || into > HALF_TILE_WIDTH {
            continue;
        }
        // Where this column's base pixel is: the edge's own descent, which
        // is what `Face::run_at` inverts.
        let run = face.run_at(across);
        let base = match face {
            // `dy = 22 * (run - 1)` for the two edges whose apex is the
            // diamond's top vertex, `22 * run` for the two whose apex is its
            // bottom one — see `docs/lighting.md`, step 15.
            Face::North | Face::West => HALF_TILE_WIDTH * (run - 1.0),
            Face::East | Face::South => HALF_TILE_WIDTH * run,
        };
        let bottom = (base + f32::from(height) + 22.0).round() as u16;
        let top = bottom.saturating_sub(height);
        for row in top..=bottom.min(rows - 1) {
            pixels[usize::from(row) * usize::from(width) + usize::from(column)] =
                Color16(0b0_11111_00000_00000);
        }
    }
    Image::new(width, rows, pixels)
}

/// The same wall with a window cut out of it: [`silhouette`], with the rectangle
/// `hole` names made transparent.
///
/// The forward projection of a [`Hole`], and [`aperture_of`] is its inverse — the
/// same relationship [`silhouette`] has with [`facing_of`], and `pub` for the
/// same reason: the readers are the tests in other crates, and a second
/// hand-drawn window would be a second opinion about what one looks like.
///
/// It draws what the projection says: a column's pixels are cleared where the
/// hole covers that column's run, between the two heights, which comes out as a
/// parallelogram with vertical sides. A real window is not quite this — the
/// client's are arched — and that is what [`aperture_of`]'s inscribed rectangle
/// is for; what this fixture is for is the arithmetic in between.
pub fn pierced(face: Face, height: u16, hole: Hole) -> Image {
    use openshard_uofiles::color::Color16;

    let wall = silhouette(face, height);
    let (width, rows) = (wall.width(), wall.height());
    let mut pixels = wall.pixels().to_vec();
    let middle = f32::from(width) / 2.0;
    let near = f32::from(hole.near) / crate::occlusion::RUN_STEPS;
    let far = f32::from(hole.far) / crate::occlusion::RUN_STEPS;
    for column in 0..width {
        let across = f32::from(column) + 0.5 - middle;
        let run = face.run_at(across);
        if run < near || run > far {
            continue;
        }
        // This column's base pixel, the way `silhouette` placed it: the wall's
        // own bottom row here, whatever the ornament above it.
        let Some(base) = (0..rows).rev().find(|row| {
            !pixels[usize::from(*row) * usize::from(width) + usize::from(column)].is_transparent()
        }) else {
            continue;
        };
        let lift = |z: u8| f32::from(z) * Z_STEP;
        let from = f32::from(base) - lift(hole.top);
        let to = f32::from(base) - lift(hole.bottom);
        for row in from.max(0.0) as u16..=to.max(0.0) as u16 {
            pixels[usize::from(row) * usize::from(width) + usize::from(column)] = Color16::TRANSPARENT;
        }
    }
    Image::new(width, rows, pixels)
}

/// A corner's silhouette: the two faces of [`silhouette`] drawn into one
/// picture, which is what the client's own corner graphics are.
///
/// `right` must be a right-half face (`North` or `East`) and `left` a left-half
/// one (`South` or `West`) — the same pairing [`Facing::Corner`] carries, for the
/// same reason: two faces on one half would be one picture of two surfaces on one
/// edge, which is not a shape the projection can draw.
///
/// `pub` for the reason [`silhouette`] is: the GPU frame test needs the *same*
/// picture the unit tests here decide against, or it would be asserting about a
/// shape this module never sees.
pub fn corner_silhouette(right: Face, left: Face, height: u16) -> Image {
    let (a, b) = (silhouette(right, height), silhouette(left, height));
    // The two are the same size by construction — same width, same `height + 45`
    // — so this is a pixel-for-pixel union with no placement in it.
    let pixels = a
        .pixels()
        .iter()
        .zip(b.pixels())
        .map(|(over, under)| match over.is_transparent() {
            true => *under,
            false => *over,
        })
        .collect();
    Image::new(a.width(), a.height(), pixels)
}

/// Which way a stepped prism climbs, and how high each of its treads stands.
///
/// **The shape a stair actually is**, and the one the client's own art draws:
/// a height field over the tile that varies along *one* axis and is constant
/// across the other — a profile, extruded. See `docs/lighting.md`'s backlog,
/// "found on a staircase in Britain".
///
/// It is a model of the whole tile rather than of a surface on one of its edges,
/// which is what makes it a different kind of answer from [`Facing`]: a wall says
/// *which edge I stand on*, and a prism says *what shape fills me*. A box is the
/// degenerate case with one tread, and every climbable static in the client's
/// files is one of the two.
/// It carries its treads in a fixed array rather than a `Vec`, and the fields are
/// private behind [`Prism::new`]: a shape rides on
/// [`Shape`](crate::occlusion::Shape), which is copied down the whole path from
/// the art table to the grid, and an allocation there would be one per static per
/// frame. [`MAX_TREADS`] is the cap and it is a cap on the *measurement* — three
/// is the most any of the client's stairs draws.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Prism {
    /// The side of the tile the climb ends at: the high side.
    ///
    /// A [`Face`] and not a direction of its own, because it is the same four
    /// edges everything else here names, and because the two faces a camera can
    /// see are the two a stair is nearly always drawn climbing towards.
    up: Face,
    /// How tall each tread stands above the static's own base, in `z`, in the
    /// order they are climbed. Only the first [`Prism::treads`] entries mean
    /// anything.
    heights: [u8; MAX_TREADS as usize],
    /// How many of them there are: at least one, at most [`MAX_TREADS`].
    count: u8,
}

// **`SEAM_OVERLAP` lived here**, `0.15` of a `z` unit, and every riser was grown
// by it at both ends. It was there to close a hairline of the enclosing sprite's
// own flat shading surviving along the tread/riser edge (`docs/gbuffer.md`'s
// Geometry section), on the reading that the rasteriser assigns a coincident
// edge's pixels to neither triangle.
//
// **Removed, because the hairline is not there.** A tread's top and its own
// riser meet at an edge built from the same `lo`/`hi` arithmetic on both sides,
// so their shared corners are bit-identical in world space, and
// `statics::push_mesh` projects a corner with a pure function of that corner —
// two identical corners cannot land on two screen positions. That makes the tie
// watertight by the fill rule rather than by luck, which is what
// `a_tread_and_its_riser_share_an_edge_bit_for_bit` now states, and
// `examples/synthetic_stair`'s face map is what measured it: **zero** pixels
// inside the flight's silhouette belong to no face, over four climb directions ×
// four zoom notches × five tread profiles — thirty-six renders, and the tread
// count is what moves the seam's own sub-pixel phase.
//
// What it cost while it stood: 1120 pixels of a single flight drawn *outside*
// their own plane's span, which is a one-pixel dark hairline across every lit
// tread (the riser winning the depth tie over the tread it stands on), and every
// step's corner displaced by `2.4` px at `4:1` in both directions. The real
// hairline was the *outer* silhouette, which is [`WIDTH_OVERLAP`]'s own doc's
// measurement and is a different edge with a different cause — the fitted prism
// against the art's true silhouette, bordering no other face at all.

/// How far [`Prism::mesh`] grows every face's own *width* — the tile-crossing
/// edge [`Prism::footprint`] never moves with `lo`/`hi` — past the tile's own
/// unit square.
///
/// Measuring the actual leak this reproduction shows (`docs/lighting.md`
/// again) found the longest runs of it were not at a tread/riser tie at all:
/// a whole riser's own side, its full height, one screen column wide — the
/// *outer* silhouette, because that edge borders no other face to overlap with,
/// only the fitted box's own edge against the art's true silhouette
/// (`best_prism`'s score is never exactly `1.0` — `PRISM_FITS`'s own doc has the
/// numbers). The same fraction-of-a-pixel this file already reasons in.
///
/// **This is the one that has a cause a real overlap can answer**, and the
/// sentence above is what says so: the retired `SEAM_OVERLAP` beside it was
/// aimed at the tread/riser tie, which borders another face and is watertight
/// without help. Here there is nothing on the other side of the edge but the
/// sprite, and the two silhouettes genuinely differ. It is still a fudge — it
/// draws a two-pixel tooth at `4:1` — and nobody has measured the sliver it
/// hides against the tooth it draws; see `docs/lighting_height.md`'s backlog.
const WIDTH_OVERLAP: f64 = 0.03;

/// Grows a footprint's own tile-crossing edge by [`WIDTH_OVERLAP`] — the pair
/// [`Prism::footprint`] holds at the tile's unit square regardless of `lo`/
/// `hi`, which for [`Face::North`]/[`Face::South`] is `x` and for
/// [`Face::East`]/[`Face::West`] is `y`. The `lo`/`hi` pair is left exactly as
/// `footprint` returned it: that edge is the tread/riser tie, built from
/// arithmetic both sides share and watertight because of it, and widening it
/// here would just move the tie rather than close anything.
fn widen_footprint(up: Face, min_x: f64, max_x: f64, min_y: f64, max_y: f64) -> (f64, f64, f64, f64) {
    match up {
        Face::North | Face::South => (min_x - WIDTH_OVERLAP, max_x + WIDTH_OVERLAP, min_y, max_y),
        Face::East | Face::West => (min_x, max_x, min_y - WIDTH_OVERLAP, max_y + WIDTH_OVERLAP),
    }
}

impl Prism {
    /// A prism from a profile, or `None` if the profile is empty or longer than
    /// [`MAX_TREADS`].
    ///
    /// The treads divide the tile evenly along the climb: `n` of them means each
    /// covers `1/n` of the run.
    pub fn new(up: Face, treads: &[u8]) -> Option<Self> {
        if treads.is_empty() || treads.len() > MAX_TREADS as usize {
            return None;
        }
        let mut heights = [0; MAX_TREADS as usize];
        heights[..treads.len()].copy_from_slice(treads);
        Some(Self {
            up,
            heights,
            count: treads.len() as u8,
        })
    }

    /// A box: one tread across the whole tile.
    ///
    /// The shape `0x071E` is — a cube five `z` tall — and the fallback for any
    /// climbable static whose treads cannot be read off its picture.
    pub fn box_of(height: u8) -> Self {
        // The high side of a box is every side, so this names the one a camera
        // sees. Nothing reads it for a single tread: the profile is constant, so
        // which way it runs cannot be observed.
        //
        // One tread is always a legal profile, which is what the `unwrap` says.
        Self::new(Face::East, &[height]).unwrap()
    }

    /// Which side of the tile it climbs towards.
    pub fn up(self) -> Face {
        self.up
    }

    /// Its profile: one height per tread, in the order they are climbed.
    pub fn treads(&self) -> &[u8] {
        &self.heights[..usize::from(self.count)]
    }

    /// How tall the prism stands where a fraction `run` along the climb is,
    /// `run` counted from the low side to the high one.
    ///
    /// The profile itself, sampled: the tread a point falls on, and its height.
    /// Saturating at both ends, because a sprite draws a sliver of itself past
    /// the tile's own edge and those pixels belong to the end tread.
    pub fn height_at(&self, run: f32) -> u8 {
        let treads = self.treads();
        // `treads` is never empty — `new` refuses an empty profile — so the index
        // is in range and the `min` is what keeps `run == 1.0` from stepping off
        // the end.
        let index = ((run.clamp(0.0, 1.0) * treads.len() as f32) as usize).min(treads.len() - 1);
        treads[index]
    }

    /// The tallest it stands. What a tile's occluder spans, and how tall a
    /// picture of it has to be.
    pub fn top(&self) -> u8 {
        // Same invariant as `height_at`: never empty.
        self.treads().iter().copied().max().unwrap()
    }

    /// The tile-relative footprint of one climb-axis strip, `lo..=hi` — both in
    /// `0.0..=1.0`, the run fraction from the low side to [`Prism::up`]. Shared by
    /// [`Prism::mesh`] (a strip, `lo < hi`, for a tread's top) and a riser's
    /// boundary plane (`lo == hi`, degenerate on the climb axis rather than a span
    /// of it) — and by [`crate::occlusion::Solid`]'s own tread boxes, which is the
    /// whole reason this lives here rather than beside either caller: `gbuffer.md`
    /// step 4c found occlusion and render asking the same question of a tread —
    /// where its strip sits — and answering it twice was two chances for the two
    /// to disagree. Moved verbatim from `occlusion::Solid::strip_footprint`,
    /// which this replaces; the occlusion tests that exercised it are the
    /// regression.
    ///
    /// `up` is `North`/`South` for a climb along `y`, `East`/`West` for one along
    /// `x`, and the strip is flat on that axis, full width on the other. `up`
    /// names the high side, so `run = 0` sits at the opposite edge and climbs
    /// towards `up` as the fraction grows.
    pub(crate) fn footprint(x: f64, y: f64, up: Face, lo: f64, hi: f64) -> (f64, f64, f64, f64) {
        let (mut min_x, mut max_x, mut min_y, mut max_y) = (x, x + 1.0, y, y + 1.0);
        match up {
            Face::North => {
                min_y = y + 1.0 - hi;
                max_y = y + 1.0 - lo;
            }
            Face::South => {
                min_y = y + lo;
                max_y = y + hi;
            }
            Face::West => {
                min_x = x + 1.0 - hi;
                max_x = x + 1.0 - lo;
            }
            Face::East => {
                min_x = x + lo;
                max_x = x + hi;
            }
        }
        (min_x, max_x, min_y, max_y)
    }

    /// This prism's honest geometry, standing on tile `(x, y)` with its own base
    /// at `base_z` — the render-side twin of [`Builder::add`](crate::occlusion::Builder::add)'s
    /// climbable branch, and built from the same two facts that branch reads:
    /// [`Prism::treads`] and [`Prism::up`].
    ///
    /// Two [`crate::mesh::Face`]s per tread, in climb order — a top and a riser,
    /// `docs/gbuffer.md` decision 3's "seven honest normals" minus the lid
    /// static's own top, which is an ordinary flat sprite and needs no mesh at
    /// all. The top is a lid, flat at the tread's own height, normal `[0, 0, 1]`
    /// — honestly, not as a blend's `k == 0` special case: a tread's own top
    /// polygon really is flat, and the *ramp* a flight of them reads as on
    /// screen was always a property of the flight, not of any one tread's own
    /// geometry. `docs/gbuffer.md` step 5 retired the blend a former
    /// `Prism::tread_normal` computed to fake that ramp's continuity on a
    /// single fixed surface tag, once this method gave every tread's top and
    /// riser its own honest normal to be measured against instead — see that
    /// step for the reproduction. The riser is the plane between
    /// this tread and the one before it (or the static's own base, for the
    /// first), facing away from `up` — the side a climber sees approaching from
    /// below, the same direction `occlusion::Solid::tread_riser_box_of`'s own doc
    /// names.
    ///
    /// **A tread's top and its own riser meet exactly, and that is what closes
    /// the seam.** Both sides of the shared edge are built from the same `lo`
    /// and the same `top_z` — [`Prism::footprint`] with `(lo, hi)` for the top
    /// and with `(lo, lo)` for the riser, which is the same expression evaluated
    /// twice — so the two quads' shared corners are bit-identical in world
    /// space, and [`crate::statics::push_mesh`] projects a corner with a pure
    /// function of that corner. Two identical corners cannot land on two screen
    /// positions, so the tie is watertight by the rasteriser's own fill rule
    /// rather than by anything this method adds.
    ///
    /// Every riser used to be grown by a `SEAM_OVERLAP` of `0.15` `z` at both
    /// ends so that the last-submitted face would win that edge outright. It is
    /// gone: see the comment where the constant stood for what it was aimed at,
    /// what measured that the hairline is not at this edge, and what it cost
    /// while it stood. [`a_tread_and_its_riser_share_an_edge_bit_for_bit`] is the
    /// gate that keeps the meeting exact.
    pub fn mesh(&self, x: i32, y: i32, base_z: i32) -> crate::mesh::Mesh {
        use crate::camera::WorldSpot;
        use crate::mesh::Face as MeshFace;

        let treads = self.treads();
        let count = treads.len();
        let mut mesh = crate::mesh::Mesh::EMPTY;
        let mut low_z = base_z;
        for (index, &height) in treads.iter().enumerate() {
            let top_z = base_z + i32::from(height);
            let lo = index as f64 / count as f64;
            let hi = (index + 1) as f64 / count as f64;

            let (min_x, max_x, min_y, max_y) = Self::footprint(f64::from(x), f64::from(y), self.up, lo, hi);
            let (min_x, max_x, min_y, max_y) = widen_footprint(self.up, min_x, max_x, min_y, max_y);
            let z = f64::from(top_z);
            let top = [
                WorldSpot {
                    x: min_x,
                    y: min_y,
                    z,
                },
                WorldSpot {
                    x: max_x,
                    y: min_y,
                    z,
                },
                WorldSpot {
                    x: max_x,
                    y: max_y,
                    z,
                },
                WorldSpot {
                    x: min_x,
                    y: max_y,
                    z,
                },
            ];
            // `MAX_FACE_VERTICES` is 4 and this is a 4-corner ring, so `new`
            // never refuses it.
            mesh.push(MeshFace::new(&top, [0.0, 0.0, 1.0]).unwrap());

            let (min_x, max_x, min_y, max_y) = Self::footprint(f64::from(x), f64::from(y), self.up, lo, lo);
            let (min_x, max_x, min_y, max_y) = widen_footprint(self.up, min_x, max_x, min_y, max_y);
            // Exactly the two treads' own heights. See this method's own doc for
            // why an overlap here is not what closes the seam.
            let riser_top = f64::from(top_z);
            let riser_low = f64::from(low_z);
            let riser = [
                WorldSpot {
                    x: min_x,
                    y: min_y,
                    z: riser_top,
                },
                WorldSpot {
                    x: max_x,
                    y: max_y,
                    z: riser_top,
                },
                WorldSpot {
                    x: max_x,
                    y: max_y,
                    z: riser_low,
                },
                WorldSpot {
                    x: min_x,
                    y: min_y,
                    z: riser_low,
                },
            ];
            let [ox, oy] = self.up.outward();
            mesh.push(MeshFace::new(&riser, [-ox, -oy, 0.0]).unwrap());

            low_z = top_z;
        }
        mesh
    }
}

/// The silhouette of a [`Prism`], drawn the way the projection draws one.
///
/// The forward direction of the measurement, and the pair to [`silhouette`]: that
/// one draws a wall standing on an edge, this one draws a solid filling the tile.
/// What makes it worth having as a function rather than as a picture is that the
/// *detector* will compare a real sprite against it — a candidate prism is scored
/// by how much of its silhouette the art agrees with — so both directions have to
/// come from one statement of the shape.
///
/// # How it is drawn
///
/// Every column of the solid is a vertical run in the image: a point `(u, v)` in
/// the tile projects to a column `(u - v) * 22` and a row `(u + v - 1) * 22`, and
/// the material above it rises `Z_STEP` pixels per `z`. So the picture is a sweep
/// over the tile's own square, each sample painting one vertical run — no
/// rasteriser and no polygon, and every pixel of the answer is a point the solid
/// really contains.
///
/// The sweep is at 1/128 of a tile, which is four samples per pixel column at the
/// widest: fine enough that no column of the picture is missed, which is the only
/// property the drawing needs.
pub fn prism_silhouette(prism: &Prism) -> Image {
    use openshard_uofiles::color::Color16;

    let width = 44u16;
    // The diamond is 44 tall — its north vertex 22 rows above the tile's centre
    // row and its south vertex 22 below — and the solid lifts the whole of it by
    // its own height.
    let rows = 45 + u16::from(prism.top()) * Z_STEP as u16;
    let mut pixels = vec![Color16::TRANSPARENT; usize::from(width) * usize::from(rows)];
    // The bottom row of the picture is the diamond's south vertex, which is where
    // `statics::stand_on` puts every static's base whatever its art holds.
    let bottom = f32::from(rows) - 1.0;

    const SAMPLES: i32 = 128;
    for i in 0..=SAMPLES {
        for j in 0..=SAMPLES {
            let (u, v) = (i as f32 / SAMPLES as f32, j as f32 / SAMPLES as f32);
            // How far along the climb this sample is. The high side is `up`, and
            // the run counts towards it.
            let run = match prism.up {
                Face::East => u,
                Face::West => 1.0 - u,
                Face::South => v,
                Face::North => 1.0 - v,
            };
            let height = f32::from(prism.height_at(run));
            let across = (u - v) * HALF_TILE_WIDTH;
            let column = (across + f32::from(width) / 2.0).floor();
            if column < 0.0 || column >= f32::from(width) {
                continue;
            }
            // The row this column of material stands on, and the row its top is
            // at: `down` from the tile's centre row, measured off the bottom of
            // the picture.
            let down = (u + v - 1.0) * HALF_TILE_WIDTH;
            let foot = bottom + down - HALF_TILE_WIDTH;
            let head = foot - height * Z_STEP;
            for row in head.max(0.0).round() as u16..=foot.max(0.0).round() as u16 {
                if row >= rows {
                    continue;
                }
                pixels[usize::from(row) * usize::from(width) + column as usize] =
                    Color16(0b0_11111_00000_00000);
            }
        }
    }
    Image::new(width, rows, pixels)
}

/// How closely a picture has to match a prism's silhouette before it is that
/// prism.
///
/// Measured rather than chosen — `tests/prism.rs` runs the comparison against a
/// real install: the two stair statics of the staircase this model came from fit
/// at **0.977** and **0.975**, and a plain wall, which is not a prism at all,
/// fits its best candidate at **0.812**. Nine tenths sits in that gap with room
/// on both sides.
///
/// The gap is also why the fit is not the only gate. A measure that scores a wall
/// at 0.81 *likes* walls, so what admits a prism is the client's own `CLIMBABLE`
/// bit first and this score second — the order-of-policy [`crate::place::Stance`]
/// already uses for a floor.
pub const PRISM_FITS: f32 = 0.9;

/// The tallest prism [`prism_of`] will consider, in `z`. Twenty is a wall's
/// height and taller than any climbable static the client ships.
const MAX_PRISM: u8 = 20;

/// And the most treads a prism may have. Four: `0x0736`'s three are the most any
/// of the client's stairs draws, and a fourth is one more than that rather than a
/// limit anything has been seen to want.
///
/// It is a cap on the *model*, which is why it is public: it is what makes a
/// [`Prism`] a fixed-size `Copy` value with no allocation behind it.
pub const MAX_TREADS: u16 = 4;

/// What solid this picture is of, or `None` where no prism is a good enough
/// likeness of it.
///
/// The inverse of [`prism_silhouette`], and the same relationship [`facing_of`]
/// has with [`silhouette`]: the shape is stated once, in the forward direction,
/// and the measurement is a search over it. There is no separate reading of the
/// pixels to drift from the drawing.
///
/// # Why a search and not a reading
///
/// A wall's face can be read straight off the art because its base is a *line*,
/// and a line has two ends to fit. A solid's silhouette is the union of every
/// column of material in it, so the top contour at one screen column is a maximum
/// over a whole diagonal of the tile — invertible only by assuming the very
/// profile that is being looked for. Scoring candidates asks the question
/// forwards, and the answer comes with a number saying how well it did, which a
/// reading never has.
///
/// # What it costs
///
/// A few hundred silhouettes per picture, which is milliseconds and **not a
/// per-frame cost**: this is `openshard-client-artscan`'s kind of work, measured
/// once into an `ArtTable` beside the client. Decision 31 is the argument.
pub fn prism_of(image: &Image) -> Option<Prism> {
    let (best, score) = best_prism(image);
    (score >= PRISM_FITS).then_some(best)
}

/// The best-fitting prism for a picture and how much of it agrees, whether or not
/// that is enough to be an answer.
///
/// What the tools call: a person looking at a graphic wants the score even when
/// it is 0.4, because a refusal with no number attached cannot be argued with.
/// [`prism_of`] is this with [`PRISM_FITS`] applied.
pub fn best_prism(image: &Image) -> (Prism, f32) {
    let drawn = drawn_count(image);
    let mut best = (Prism::box_of(0), 0.0);
    for candidate in candidates() {
        // **An exact bound, not a heuristic.** The score is
        // `both / either`, and whatever the two silhouettes overlap, `both` is at
        // most the smaller drawn count and `either` at least the larger — so
        // `min / max` is a ceiling on what scoring this candidate could return.
        // A candidate that cannot beat the best already found is skipped without
        // its pixels being walked, and no candidate that could beat it is.
        //
        // It is most of the search: the drawn counts are dominated by height, so
        // a picture ten rows tall dismisses every prism twice its size on one
        // division. Nothing about the answer changes — `tests/prism.rs` scores
        // the same stairs at the same numbers.
        let (low, high) = (drawn.min(candidate.drawn), drawn.max(candidate.drawn));
        let ceiling = match high {
            0 => 0.0,
            _ => low as f32 / high as f32,
        };
        if ceiling <= best.1 {
            continue;
        }
        let score = silhouettes_agree(image, &candidate.silhouette);
        if score > best.1 {
            best = (candidate.prism, score);
        }
    }
    best
}

/// The most blocks a [`crate::occlusion::Shape`] may be authored with —
/// mirroring [`MAX_TREADS`]'s own discipline: a cap on the *model*, not a limit
/// anything has been seen to want. Three names every block an arch needs — two
/// posts and a lintel — with one spare.
pub const MAX_BLOCKS: u16 = 4;

/// One axis-aligned box in a graphic's own tile-local coordinates: a post, a
/// lintel, one leaf of a shape [`Prism`]'s single climb profile cannot
/// describe. An arch is a post, a post and a lintel — the gap between the two
/// posts states nothing, because a gap is simply the absence of a third block.
///
/// `x` and `y` are eighths of the tile, `0..=8` — coarser than a hole's 255ths
/// (step 21.3 of `docs/lighting.md`) or the 128-sample sweep
/// [`blocks_silhouette`] draws with, because a person places these by eye
/// against a silhouette rather than measuring a pixel edge, and a block a
/// person cannot state in eighths is not one a text file should pretend to
/// carry precisely. `z` is the same axis [`Prism::treads`] already measures
/// in: `z` above the static's own base.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Block {
    /// `(min, max)`, both `0..=8`, `min < max`.
    pub x: (u8, u8),
    /// `(min, max)`, both `0..=8`, `min < max`.
    pub y: (u8, u8),
    /// `(min, max)`, `min < max`, in `z`.
    pub z: (u8, u8),
}

impl Block {
    /// `None` for an empty or an out-of-range span — the invariant a
    /// hand-written row could otherwise break, the same refusal [`Prism::new`]
    /// makes for its own treads.
    pub fn new(x: (u8, u8), y: (u8, u8), z: (u8, u8)) -> Option<Self> {
        if x.0 >= x.1 || y.0 >= y.1 || z.0 >= z.1 || x.1 > 8 || y.1 > 8 {
            return None;
        }
        Some(Self { x, y, z })
    }

    /// The footprint as fractions of the tile, `0.0..=1.0` on each axis.
    fn footprint(self) -> (f32, f32, f32, f32) {
        (
            f32::from(self.x.0) / 8.0,
            f32::from(self.x.1) / 8.0,
            f32::from(self.y.0) / 8.0,
            f32::from(self.y.1) / 8.0,
        )
    }
}

/// A shape's blocks, held the way [`Prism`] holds its treads: a fixed array and
/// a count, so a [`crate::occlusion::Shape`] carrying some is still `Copy` and
/// costs no allocation on the path from the table to the grid.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Blocks {
    list: [Block; MAX_BLOCKS as usize],
    count: u8,
}

impl Blocks {
    /// No blocks — every graphic's state before a person authors one.
    pub const EMPTY: Self = Self {
        list: [Block {
            x: (0, 0),
            y: (0, 0),
            z: (0, 0),
        }; MAX_BLOCKS as usize],
        count: 0,
    };

    /// A list of blocks, or `None` if there are more than [`MAX_BLOCKS`]. Empty
    /// is legal and is [`Blocks::EMPTY`].
    pub fn new(blocks: &[Block]) -> Option<Self> {
        if blocks.len() > MAX_BLOCKS as usize {
            return None;
        }
        let mut list = Self::EMPTY.list;
        list[..blocks.len()].copy_from_slice(blocks);
        Some(Self {
            list,
            count: blocks.len() as u8,
        })
    }

    /// The blocks, in the order they were authored.
    pub fn blocks(&self) -> &[Block] {
        &self.list[..usize::from(self.count)]
    }

    /// Whether nothing has been authored.
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }
}

impl Default for Blocks {
    fn default() -> Self {
        Self::EMPTY
    }
}

/// The silhouette of a [`Blocks`] list, drawn the way [`prism_silhouette`]
/// draws one solid: a sweep over the tile, each block painting the vertical
/// run its own height fills within its own footprint.
///
/// Unlike a [`Prism`], a block need not touch the ground: two blocks may draw
/// the same column at different heights, a lintel floating over the gap
/// between two posts, and each is swept on its own rather than assuming
/// material fills everything below its top the way a climb profile does.
pub fn blocks_silhouette(blocks: &Blocks) -> Image {
    use openshard_uofiles::color::Color16;

    let width = 44u16;
    let top = blocks.blocks().iter().map(|block| block.z.1).max().unwrap_or(0);
    let rows = 45 + u16::from(top) * Z_STEP as u16;
    let mut pixels = vec![Color16::TRANSPARENT; usize::from(width) * usize::from(rows)];
    let bottom = f32::from(rows) - 1.0;

    const SAMPLES: i32 = 128;
    for i in 0..=SAMPLES {
        for j in 0..=SAMPLES {
            let (u, v) = (i as f32 / SAMPLES as f32, j as f32 / SAMPLES as f32);
            let across = (u - v) * HALF_TILE_WIDTH;
            let column = (across + f32::from(width) / 2.0).floor();
            if column < 0.0 || column >= f32::from(width) {
                continue;
            }
            let down = (u + v - 1.0) * HALF_TILE_WIDTH;
            let foot = bottom + down - HALF_TILE_WIDTH;
            for block in blocks.blocks() {
                let (min_x, max_x, min_y, max_y) = block.footprint();
                if u < min_x || u > max_x || v < min_y || v > max_y {
                    continue;
                }
                let head = foot - f32::from(block.z.1) * Z_STEP;
                let base = foot - f32::from(block.z.0) * Z_STEP;
                for row in head.max(0.0).round() as u16..=base.max(0.0).round() as u16 {
                    if row >= rows {
                        continue;
                    }
                    pixels[usize::from(row) * usize::from(width) + column as usize] =
                        Color16(0b0_11111_00000_00000);
                }
            }
        }
    }
    Image::new(width, rows, pixels)
}

/// One prism [`best_prism`] scores a picture against, with its silhouette
/// already drawn and counted.
struct Candidate {
    prism: Prism,
    silhouette: Image,
    /// How many pixels that silhouette draws inside the 44-wide tile — the
    /// `either` term's floor and the `both` term's ceiling, see [`best_prism`].
    drawn: u32,
}

/// Every prism the search considers, in the order it considers them.
///
/// **The candidate set does not depend on the picture.** It is `MAX_PRISM` boxes
/// and one flight of stairs per (face, treads, top), and drawing one silhouette
/// samples the tile 129×129 times — so drawing them per graphic is 261 of those
/// per picture, against 39,189 pictures in a real install, to redraw the same 261
/// shapes 39,189 times. That is the difference between a scan measured in seconds
/// and one that does not finish, and it is what the atlas pays on the render
/// thread when there is no table beside the install to read instead
/// (`crate::occlusion::Shape::of`).
///
/// A `OnceLock` here is a memo and not state: the table is a pure function of the
/// constants above it, every caller sees the same one, and nothing can write to
/// it. The workspace's rule is about a *world* nothing owns.
fn candidates() -> &'static [Candidate] {
    static CANDIDATES: std::sync::OnceLock<Vec<Candidate>> = std::sync::OnceLock::new();
    CANDIDATES.get_or_init(|| {
        let mut candidates = Vec::new();
        let mut push = |prism: Prism| {
            let silhouette = prism_silhouette(&prism);
            candidates.push(Candidate {
                prism,
                drawn: drawn_count(&silhouette),
                silhouette,
            });
        };
        for height in 0..=MAX_PRISM {
            push(Prism::box_of(height));
        }
        for up in [Face::North, Face::East, Face::South, Face::West] {
            for treads in 2..=MAX_TREADS {
                for top in 1..=u16::from(MAX_PRISM) {
                    // An even climb: the treads rise in equal steps to `top`,
                    // which is how every stair the client draws is built. An
                    // uneven profile is a search this does not make until a
                    // graphic wants one — and a graphic that wants one shows up
                    // as a *score*, not as a silent wrong answer.
                    let profile: Vec<u8> = (1..=treads).map(|i| (top * i / treads) as u8).collect();
                    // The profile is `treads` long and `treads` runs to
                    // `MAX_TREADS`, so this is a legal prism by construction.
                    push(Prism::new(up, &profile).unwrap());
                }
            }
        }
        candidates
    })
}

/// How much two silhouettes agree: the drawn pixels they share over the drawn
/// pixels either of them has.
///
/// Lined up by the **bottom row** and the **centre column**, which is not a fit
/// parameter but where the client itself puts a sprite —
/// [`statics::stand_on`](crate::statics::stand_on) stands every static on the
/// bottom edge of its picture. A measure that slid one picture over the other
/// until it liked what it saw would have a free variable nobody stated.
///
/// Both directions of disagreement count. A model smaller than the art leaves
/// drawn pixels with no surface under them; a model bigger than the art puts
/// surface where the artist drew air, and that one is worse — it is a shadow with
/// nothing in the picture casting it.
///
/// `pub` for [`best_prism`]'s own use and for `tests/author.rs`, step 23.4's
/// instrument: scoring a hand-placed [`Blocks`] candidate against the art is the
/// same comparison, and a second copy of the alignment rule would be a second
/// place for it to drift from this one.
pub fn silhouettes_agree(art: &Image, model: &Image) -> f32 {
    let rows = art.height().max(model.height());
    let (mut both, mut either) = (0u32, 0u32);
    for column in 0..44u16 {
        for row in 0..rows {
            let (a, b) = (drawn_at(art, column, row), drawn_at(model, column, row));
            both += u32::from(a && b);
            either += u32::from(a || b);
        }
    }
    match either {
        0 => 0.0,
        _ => both as f32 / either as f32,
    }
}

/// How many pixels a picture draws inside the 44-wide tile, counted the way
/// [`silhouettes_agree`] counts them: bottom row aligned, centre column aligned,
/// and whatever falls outside the tile's width not counted at all.
///
/// It is one of the two silhouettes' side of that comparison on its own, which is
/// what makes it a bound on the comparison — see [`best_prism`].
fn drawn_count(image: &Image) -> u32 {
    let mut drawn = 0;
    for column in 0..44u16 {
        for row in 0..image.height() {
            drawn += u32::from(drawn_at(image, column, row));
        }
    }
    drawn
}

/// Whether a picture draws anything `row` rows up from its own bottom edge, in
/// the column `column` of a 44-wide tile.
///
/// A graphic narrower than a tile is centred on the tile's column, which is where
/// the client draws one.
fn drawn_at(image: &Image, column: u16, row: u16) -> bool {
    use openshard_uofiles::color::Color16;

    if row >= image.height() {
        return false;
    }
    let offset = (44 - i32::from(image.width())) / 2;
    let x = i32::from(column) - offset;
    if x < 0 || x >= i32::from(image.width()) {
        return false;
    }
    !image
        .pixel(x as u16, image.height() - 1 - row)
        .unwrap_or(Color16::TRANSPARENT)
        .is_transparent()
}

#[cfg(test)]
mod tests {
    use openshard_uofiles::color::Color16;

    use super::*;

    /// The single-face answer, for the tests that are about a plain wall.
    ///
    /// A corner is a *panic* and not a `None`: every test below that asserts
    /// "undecided" is asserting that the picture is not a surface this can name,
    /// and a corner is one. Folding the two together would let the corner pass
    /// quietly start answering about a post.
    fn face_of(image: &Image) -> Option<Face> {
        match facing_of(image) {
            Some(Facing::One(face)) => Some(face),
            Some(corner) => panic!("{corner:?}, and the fixture is not a corner"),
            None => None,
        }
    }

    /// The tile is the camera's tile and not a second opinion about it.
    #[test]
    fn a_tile_is_the_width_the_camera_draws_one_at() {
        assert_eq!(TILE_WIDTH as i32, crate::camera::TILE_WIDTH);
    }

    /// Each of the four, told apart from a picture of it. The property that
    /// matters is that all four are distinguished — a detector that answered
    /// `North` always would pass any one of these on its own.
    #[test]
    fn each_face_is_read_back_off_its_own_silhouette() {
        for face in [Face::North, Face::East, Face::South, Face::West] {
            assert_eq!(face_of(&silhouette(face, 80)), Some(face), "{face:?}");
        }
    }

    /// A corner is two faces at once, and both of them are read.
    ///
    /// `0x0104` is the client's own, and this is the shape of it: every column
    /// of the tile drawn, with the base descending both ways from the middle.
    /// What used to happen is in the name of the test this replaced — whichever
    /// half was proposed, `SPILL` refused it for the other one, and the whole
    /// tile came back `Upright`.
    ///
    /// All four pairings, because a detector that answered `Corner { East,
    /// South }` always — which is the only pairing a real client graphic is —
    /// would pass a test of the one.
    #[test]
    fn a_corner_is_both_of_its_faces() {
        for right in [Face::North, Face::East] {
            for left in [Face::South, Face::West] {
                assert_eq!(
                    facing_of(&corner_silhouette(right, left, 80)),
                    Some(Facing::Corner { right, left }),
                    "{right:?} and {left:?}",
                );
            }
        }
    }

    /// And a pixel of it belongs to the face drawn on its own half.
    ///
    /// The rule `statics.wgsl` applies per fragment: a corner's two surfaces are
    /// two halves of one picture, so which half a pixel is on is which surface it
    /// is a pixel of. Without it a corner would have to pick one of its faces for
    /// the whole tile, which is a wall shaded along an axis half of it does not
    /// run on.
    #[test]
    fn a_pixel_of_a_corner_belongs_to_the_face_on_its_own_half() {
        let corner = Facing::Corner {
            right: Face::East,
            left: Face::South,
        };
        assert_eq!(corner.on_half(10.0), Face::East);
        assert_eq!(corner.on_half(-10.0), Face::South);
        // And a plain wall is the same face wherever the pixel is, including the
        // sliver of its own thickness drawn past the edge it stands on.
        assert_eq!(Facing::One(Face::East).on_half(-4.0), Face::East);
    }

    /// A picture with one face on it and a blob on the other half is not a
    /// corner.
    ///
    /// The property the second pass rests on: a corner is two *faces*, and a
    /// half that fails every gate is not made into one by the half beside it
    /// passing. Without this the corner pass would be a way of letting anything
    /// wide enough through — the `SPILL` gate exists precisely because a
    /// half-full picture is not a wall.
    #[test]
    fn a_face_beside_a_blob_is_not_a_corner() {
        let wall = silhouette(Face::East, 80);
        let (width, height) = (wall.width(), wall.height());
        let mut pixels = wall.pixels().to_vec();
        // A square block filling the left half of the tile's column: it covers
        // the half, it stands up, and its base is level, so no 45° run fits it.
        for row in height - 40..height {
            for column in 2..21u16 {
                pixels[usize::from(row) * usize::from(width) + usize::from(column)] =
                    Color16(0b0_11111_00000_00000);
            }
        }
        assert_eq!(facing_of(&Image::new(width, height, pixels)), None);
    }

    /// A post covers no edge: a few columns at the tile's centre with a level
    /// base. `0x0101` is the client's, and the gate is that nothing 45° can be
    /// fitted through a level line.
    #[test]
    fn a_post_is_undecided() {
        let width = 44u16;
        let rows = 90u16;
        let mut pixels = vec![Color16::TRANSPARENT; usize::from(width) * usize::from(rows)];
        for column in 18..26u16 {
            for row in 4..86u16 {
                pixels[usize::from(row) * usize::from(width) + usize::from(column)] =
                    Color16(0b0_11111_00000_00000);
            }
        }
        assert_eq!(face_of(&Image::new(width, rows, pixels)), None);
    }

    /// A wall's own thickness, drawn past the edge it stands on, does not stop it
    /// being read — and enough of it does.
    ///
    /// This is the tolerance a real graphic needs: `0x0100` draws 3.5 pixels of
    /// its far side past the tile's centre column, `0x0063` draws 8.5 because it
    /// is low enough that you look down on its top, and a detector that demanded
    /// an empty half would refuse most of the walls a city is built out of. The
    /// second half of the test is what keeps that tolerance from swallowing the
    /// corner above — the two are stated together because loosening one without
    /// looking at the other is exactly how this gate stops working.
    #[test]
    fn a_sliver_of_thickness_is_allowed_and_a_second_face_is_not() {
        for by in [4, 10] {
            let sliver = smeared(&silhouette(Face::East, 80), by);
            assert_eq!(face_of(&sliver), Some(Face::East), "{by} pixels of thickness");
        }
        // Twenty pixels is not thickness, and it is not a corner either: what is
        // drawn on the other half is this wall's own base line moved sideways, so
        // it sits twenty pixels off the edge it would have to stand on and the
        // position gate refuses it. A corner is two faces each on *its own* edge.
        let wide = smeared(&silhouette(Face::East, 80), 20);
        assert_eq!(facing_of(&wide), None, "twenty pixels is another face");
    }

    /// The same picture with every drawn column copied `by` columns to its left,
    /// which is what a wall's thickness looks like on the far side of its edge.
    fn smeared(image: &Image, by: u16) -> Image {
        let (width, height) = (image.width(), image.height());
        let mut pixels = image.pixels().to_vec();
        for y in 0..height {
            for x in by..width {
                let from = usize::from(y) * usize::from(width) + usize::from(x);
                let to = from - usize::from(by);
                if !pixels[from].is_transparent() && pixels[to].is_transparent() {
                    pixels[to] = pixels[from];
                }
            }
        }
        Image::new(width, height, pixels)
    }

    /// A picture in the right *shape* somewhere else in the tile is not a wall.
    ///
    /// The gate the slope cannot supply: a 45° line has the same direction
    /// wherever it sits, and a wall's base has nowhere to sit but the tile edge.
    /// `0x0171` is the client's own case — a flat diamond drawn eighty pixels
    /// above its own tile, an awning or a roof, whose lower-right side is a
    /// clean run in the right half with the left half empty. It passed every
    /// other gate here and was shaded as a vertical face.
    ///
    /// Lifting is the honest mutation for it: the same silhouette, the same
    /// slope, the same straightness, the same standing height, moved. Only the
    /// position test can tell the two apart.
    #[test]
    fn a_wall_shaped_picture_off_its_tile_s_edge_is_undecided() {
        let wall = silhouette(Face::East, 60);
        assert_eq!(face_of(&wall), Some(Face::East), "the fixture is not a wall");
        assert_eq!(
            face_of(&lifted(&wall, 2)),
            Some(Face::East),
            "two pixels is rounding"
        );
        for by in [6, 20, 40] {
            assert_eq!(
                face_of(&lifted(&wall, by)),
                None,
                "lifted {by} pixels off its edge"
            );
        }
    }

    /// The same picture with everything drawn in it moved `by` rows up the image,
    /// which is what an awning drawn above its own tile looks like.
    fn lifted(image: &Image, by: u16) -> Image {
        let (width, height) = (image.width(), image.height());
        let mut pixels = vec![Color16::TRANSPARENT; usize::from(width) * usize::from(height)];
        for y in by..height {
            for x in 0..width {
                pixels[usize::from(y - by) * usize::from(width) + usize::from(x)] =
                    image.pixel(x, y).unwrap();
            }
        }
        Image::new(width, height, pixels)
    }

    /// `0x003C`'s hole, which is what the detector reads off the client's own
    /// window: the middle third of the tile, from ten `z` above the sill to
    /// fifteen. Every fixture below is a variation on it, so that a number that
    /// moves can be compared with a real one.
    const WINDOW: Hole = Hole {
        near: 93,
        far: 185,
        bottom: 10,
        top: 15,
    };

    /// A window is read back off its own picture, on every face.
    ///
    /// All four for the reason [`each_face_is_read_back_off_its_own_silhouette`]
    /// tests all four: the run is measured along the face's own axis and it
    /// counts *the other way* on two of them, so a detector that had the sign
    /// wrong would read a window at one end of the wall as a window at the other
    /// — and a test of one face would never say so.
    #[test]
    fn a_window_is_read_back_off_its_own_silhouette() {
        for face in [Face::North, Face::East, Face::South, Face::West] {
            let window = pierced(face, 80, WINDOW);
            assert_eq!(
                facing_of(&window),
                Some(Facing::One(face)),
                "{face:?}: a wall with a window in it is still a wall",
            );
            assert_eq!(aperture_of(&window, Facing::One(face)), Some(WINDOW), "{face:?}");
        }
    }

    /// And a wall with nothing cut out of it has no hole. The other half of the
    /// property above, and the one that says the detector is not answering
    /// `Some` at whatever it is shown.
    #[test]
    fn a_solid_wall_has_no_hole() {
        for face in [Face::North, Face::East, Face::South, Face::West] {
            let wall = silhouette(face, 80);
            assert_eq!(aperture_of(&wall, Facing::One(face)), None, "{face:?}");
        }
    }

    /// A corner is refused, whatever its picture holds.
    ///
    /// Two faces in one picture, and a hole given to a corner would be given to
    /// *both* of its panels — a window in the wall it is cut into and a window in
    /// the wall beside it. Nothing in a silhouette says which half a hole belongs
    /// to, so this says nothing. The fixture is a picture with a real hole in it,
    /// because the refusal has to be about the corner rather than about there
    /// being nothing to find.
    #[test]
    fn a_corner_is_refused_a_hole() {
        let window = pierced(Face::East, 80, WINDOW);
        assert_eq!(
            aperture_of(
                &window,
                Facing::Corner {
                    right: Face::East,
                    left: Face::South,
                },
            ),
            None,
        );
    }

    /// A gap that runs off the end of the face is not a hole.
    ///
    /// [`HOLE_MARGIN`], and what it is defending against is a picture of two
    /// things rather than one thing with a hole in it: an arch's leg, a post
    /// beside a wall, the space between a building and the fence next to it. A
    /// window has wall all the way round it, and the run is the direction the
    /// column-by-column measurement cannot see that in.
    #[test]
    fn a_gap_at_the_end_of_a_face_is_not_a_hole() {
        for (near, far) in [(0, 128), (128, 255)] {
            let open = Hole { near, far, ..WINDOW };
            let picture = pierced(Face::East, 80, open);
            assert_eq!(
                aperture_of(&picture, Facing::One(Face::East)),
                None,
                "a gap from {near} to {far} along the run",
            );
        }
    }

    /// A scratch is not a window, in either direction.
    ///
    /// [`HOLE_MIN_RUN`] and [`HOLE_MIN_RISE`]. A stray transparent pixel inside a
    /// run of art and a one-column notch between two bricks are both real things
    /// in the client's own pictures, and either would otherwise be a slot for
    /// light to come through — which is the direction that shows: a wrong hole is
    /// *brighter* than the truth.
    #[test]
    fn a_scratch_is_not_a_window() {
        let thin = Hole {
            near: 120,
            far: 128,
            ..WINDOW
        };
        assert_eq!(
            aperture_of(&pierced(Face::East, 80, thin), Facing::One(Face::East)),
            None
        );
        let low = Hole {
            bottom: 10,
            top: 11,
            ..WINDOW
        };
        assert_eq!(
            aperture_of(&pierced(Face::East, 80, low), Facing::One(Face::East)),
            None
        );
    }

    /// Two holes in one column are refused rather than merged.
    ///
    /// A surface carries one rectangle, and a column with two gaps is a picture
    /// of something else — a lattice, a pair of arrow slits, a leaded window with
    /// its mullions drawn. Reading the two as one would open the stone between
    /// them; picking one would be picking whichever the scan met first.
    #[test]
    fn two_gaps_in_one_column_are_refused() {
        let upper = Hole {
            bottom: 16,
            top: 19,
            ..WINDOW
        };
        let both = both_holes(Face::East, 80, WINDOW, upper);
        assert_eq!(aperture_of(&both, Facing::One(Face::East)), None);
    }

    /// Two windows side by side are one refusal too: the gap columns are not one
    /// run of them, so neither is "the" hole and a surface has one.
    #[test]
    fn two_holes_along_the_run_are_refused() {
        let right = Hole {
            near: 30,
            far: 70,
            ..WINDOW
        };
        let left = Hole {
            near: 150,
            far: 200,
            ..WINDOW
        };
        let both = both_holes(Face::East, 80, right, left);
        assert_eq!(aperture_of(&both, Facing::One(Face::East)), None);
    }

    /// One wall with two rectangles cut out of it.
    fn both_holes(face: Face, height: u16, one: Hole, other: Hole) -> Image {
        let (a, b) = (pierced(face, height, one), pierced(face, height, other));
        let pixels = a
            .pixels()
            .iter()
            .zip(b.pixels())
            .map(|(over, under)| match over.is_transparent() {
                true => *over,
                false => *under,
            })
            .collect();
        Image::new(a.width(), a.height(), pixels)
    }

    /// **The rectangle is the largest one that fits**, and which way it grows is
    /// decided by area rather than by a rule about shapes.
    ///
    /// The client's windows are arches: `0x003C` is two pixels taller in the
    /// middle than at its ends, so a bounding box would let light through the
    /// stone the artist drew round the corners. Both directions are here because
    /// the trade is real — losing height to keep the width, and losing width to
    /// keep the height — and a detector that always did one of them would pass a
    /// test of the other only by accident.
    #[test]
    fn the_hole_is_the_largest_rectangle_that_fits_inside_it() {
        // An arch: the two end columns closed off above `z = 15`, where the six
        // between them are open to sixteen. Keeping all eight columns and giving
        // up the top `z` is 8 by 20 pixels; keeping the height and losing the two
        // ends is 6 by 24, which is smaller.
        let arched = filled(&pierced(Face::East, 80, TALL), Face::East, 80, &[28, 35], 15);
        assert_eq!(aperture_of(&arched, Facing::One(Face::East)), Some(WINDOW));

        // And a chimney: five of the eight closed off at twelve, so the wide
        // rectangle is 8 by 8 and the tall one is 3 by 24.
        let chimney = filled(
            &pierced(Face::East, 80, TALL),
            Face::East,
            80,
            &[28, 29, 33, 34, 35],
            12,
        );
        assert_eq!(
            aperture_of(&chimney, Facing::One(Face::East)),
            Some(Hole {
                near: 128,
                far: 162,
                bottom: 10,
                top: 16,
            }),
        );
    }

    /// [`WINDOW`] with one more `z` of height on it, which the two shapes above
    /// are cut back from.
    const TALL: Hole = Hole { top: 16, ..WINDOW };

    /// Put the wall back over part of a window: in the named columns of the
    /// picture, everything above `keep` `z` is stone again.
    ///
    /// Stated in the surface's own coordinates rather than in rows, because the
    /// base line descends: "above fifteen `z`" is a different row in every column
    /// and the same statement about the wall.
    fn filled(window: &Image, face: Face, height: u16, columns: &[u16], keep: u8) -> Image {
        let solid = silhouette(face, height);
        let (width, rows) = (window.width(), window.height());
        let mut pixels = window.pixels().to_vec();
        for column in columns {
            let base = (0..rows)
                .rev()
                .find(|row| !solid.pixel(*column, *row).unwrap().is_transparent())
                .expect("a column of the face");
            // Strictly above the row `keep` names, so that the hole's top in this
            // column is exactly `keep`.
            for row in 0..base - u16::from(keep) * Z_STEP as u16 {
                pixels[usize::from(row) * usize::from(width) + usize::from(*column)] =
                    solid.pixel(*column, row).unwrap();
            }
        }
        Image::new(width, rows, pixels)
    }

    /// A slab whose base is a clean 45° run is still not a wall.
    ///
    /// The shape a roof piece has: the right geometry along the ground and no
    /// height above it. Without the standing gate this would come back `North`
    /// and be shaded along an axis it does not run on.
    #[test]
    fn a_low_slab_is_undecided() {
        assert_eq!(face_of(&silhouette(Face::North, 6)), None);
    }

    /// Nothing drawn at all, and a picture too narrow to hold an edge. Neither is
    /// a wall and neither may panic.
    #[test]
    fn an_empty_or_narrow_picture_is_undecided() {
        assert_eq!(
            face_of(&Image::new(44, 44, vec![Color16::TRANSPARENT; 44 * 44])),
            None
        );
        assert_eq!(
            face_of(&Image::new(20, 60, vec![Color16(0b0_11111_00000_00000); 20 * 60])),
            None
        );
    }

    /// **The seam**, which is the whole reason a face is worth measuring.
    ///
    /// The end of one tile's face and the start of the next tile's along the same
    /// run name one world line. Stated in world coordinates, because that is
    /// where the lighting reads them: tile `(x, y)`'s north face at run 1 is the
    /// point `(x + 1, y)`, and tile `(x + 1, y)`'s north face at run 0 is the same
    /// point. Without this a row of wall tiles is a row of separately lit
    /// sprites, which is exactly what it looked like.
    #[test]
    fn one_tile_s_face_ends_where_the_next_one_s_begins() {
        for (face, step) in [
            (Face::North, (1.0, 0.0)),
            (Face::South, (1.0, 0.0)),
            (Face::East, (0.0, 1.0)),
            (Face::West, (0.0, 1.0)),
        ] {
            let (end_x, end_y) = face.place_at(1.0);
            let (start_x, start_y) = face.place_at(0.0);
            assert_eq!(
                (end_x, end_y),
                (start_x + step.0, start_y + step.1),
                "{face:?} does not join its neighbour along the axis it runs on",
            );
        }
    }

    /// The run is the inverse of the place, over the half the face occupies —
    /// the property `statics.wgsl` depends on, since it computes the run from a
    /// pixel's offset and the place from the run.
    #[test]
    fn a_run_and_a_place_are_one_mapping() {
        for face in [Face::North, Face::East, Face::South, Face::West] {
            for step in 0..=22u8 {
                let across = match face {
                    Face::North | Face::East => f32::from(step),
                    Face::South | Face::West => -f32::from(step),
                };
                let run = face.run_at(across);
                assert!((0.0..=1.0).contains(&run), "{face:?} at {across}: {run}");
                let (x, y) = face.place_at(run);
                // The fixed coordinate is the edge the face is, exactly — not
                // nearly. A wall's pixels are *on* the tile boundary, and a
                // fraction that drifted off it would put the lit surface inside
                // the tile.
                match face {
                    Face::North => assert_eq!(y, 0.0),
                    Face::South => assert_eq!(y, 1.0),
                    Face::East => assert_eq!(x, 1.0),
                    Face::West => assert_eq!(x, 0.0),
                }
            }
        }
    }

    /// The lowest and highest drawn pixel of every column, as an offset from the
    /// bottom of the picture — the same profile `tests/artshot.rs` prints off a
    /// real sprite, so a model's silhouette and the client's own can be compared
    /// by eye and by assertion in the same terms.
    fn profile(image: &Image) -> Vec<Option<(u16, u16)>> {
        (0..image.width())
            .map(|column| {
                let drawn: Vec<u16> = (0..image.height())
                    .filter(|row| !image.pixel(column, *row).unwrap().is_transparent())
                    .collect();
                let last = image.height() - 1;
                Some((last - *drawn.last()?, last - *drawn.first()?))
            })
            .collect()
    }

    /// A flat prism is the tile's own diamond, **filled**: a column of the
    /// picture is a vertical run and not a single pixel, because a column of the
    /// image is a whole diagonal of the tile.
    ///
    /// That is worth pinning on its own, and it is the thing the first version of
    /// this test got wrong: the base of a floor descends 1:1 towards the centre
    /// column, but what stands *above* that base is the rest of the diamond, 44
    /// rows of it at the middle and none at either end. Every prism below is this
    /// shape lifted.
    #[test]
    fn a_prism_of_no_height_is_the_tile_it_stands_in() {
        let flat = prism_silhouette(&Prism::box_of(0));
        assert_eq!(flat.width(), 44);
        assert_eq!(flat.height(), 45);
        for (column, band) in profile(&flat).iter().enumerate() {
            let (base, top) = band.expect("the diamond covers every column");
            // How far this column is from the centre of the tile's own column,
            // which is what the diamond's two edges are measured from.
            //
            // Within a pixel, and the pixel is real rather than slack: a column
            // covers a *band* of the tile's diagonal and not a line, so the two
            // end columns are two pixels of diamond rather than none. Stating it
            // exactly would be stating the sweep's rounding.
            let across = (column as i32 - 22).abs();
            assert!(
                (i32::from(base) - (across - 1).max(0)).abs() <= 1,
                "base of column {column}: {base} against {across} across",
            );
            assert!(
                (i32::from(top) - (44 - across)).abs() <= 1,
                "top of column {column}: {top} against {across} across",
            );
        }
    }

    /// A box is that diamond with `Z_STEP` pixels of side per `z` under it — in
    /// **every** column, which is the whole of what "extruded" means.
    ///
    /// Measured as the difference against the flat prism rather than against a
    /// formula: a column of the picture is a band of the tile's diagonal, so its
    /// exact depth is the sweep's rounding, and the *difference* between one
    /// height and another is not.
    #[test]
    fn a_box_is_the_diamond_raised_by_its_own_height() {
        let height = 10u8;
        let flat = profile(&prism_silhouette(&Prism::box_of(0)));
        let boxed = profile(&prism_silhouette(&Prism::box_of(height)));
        for column in 0..44 {
            let (base, top) = boxed[column].expect("a box covers every column");
            let (flat_base, flat_top) = flat[column].expect("so does the diamond");
            let grew = (i32::from(top) - i32::from(base)) - (i32::from(flat_top) - i32::from(flat_base));
            assert_eq!(
                grew,
                i32::from(height) * Z_STEP as i32,
                "the side of the box at column {column}",
            );
        }
    }

    /// A single block spanning the whole tile at `z: (0, height)` is exactly the
    /// box [`Prism::box_of`] draws — the two are the same shape stated two ways,
    /// and [`blocks_silhouette`] must agree with [`prism_silhouette`] on it
    /// pixel for pixel, not merely to a tolerance.
    #[test]
    fn one_full_tile_block_is_the_box_a_prism_draws() {
        let height = 12u8;
        let block = Block::new((0, 8), (0, 8), (0, height)).expect("the whole tile");
        let blocks = Blocks::new(&[block]).expect("one block");
        assert_eq!(
            blocks_silhouette(&blocks),
            prism_silhouette(&Prism::box_of(height))
        );
    }

    /// A block covering a quarter of the tile draws strictly less than the box
    /// that covers all of it, and never draws where that box does not — its
    /// footprint is a subset, so its silhouette is one too.
    #[test]
    fn a_partial_footprint_draws_a_subset_of_the_full_boxs_silhouette() {
        let height = 12u8;
        let corner = Block::new((0, 4), (0, 4), (0, height)).expect("one quarter of the tile");
        let blocks = Blocks::new(&[corner]).expect("one block");
        let drawn = blocks_silhouette(&blocks);
        let full = prism_silhouette(&Prism::box_of(height));
        for column in 0..44u16 {
            for row in 0..full.height() {
                if drawn_at(&drawn, column, row) {
                    assert!(
                        drawn_at(&full, column, row),
                        "column {column}, row {row}: outside the full box",
                    );
                }
            }
        }
        assert!(
            drawn_count(&drawn) < drawn_count(&full),
            "a quarter footprint draws less than the whole tile",
        );
        assert!(drawn_count(&drawn) > 0, "and it draws something");
    }

    /// Two blocks with a gap between their footprints, and a third bridging
    /// both above it — a lintel over a gap between two posts, the shape decision
    /// 41 exists for. The silhouette is the union of all three, drawn
    /// independently: nowhere does one block's presence hide another's.
    #[test]
    fn a_lintel_floats_over_the_gap_between_two_posts() {
        let post = |x: (u8, u8)| Block::new(x, (0, 8), (0, 20)).expect("a post");
        let lintel = Block::new((0, 8), (0, 8), (15, 20)).expect("the beam");
        let arch = Blocks::new(&[post((0, 2)), post((6, 8)), lintel]).expect("three blocks");
        let drawn = blocks_silhouette(&arch);
        assert!(drawn_count(&drawn) > 0, "an arch draws something");
        // Each post alone, and each post plus the lintel, both drawn as their own
        // silhouette — the union property, checked rather than assumed: every
        // pixel either alone draws is drawn in the arch, and nothing else is.
        let posts_alone = Blocks::new(&[post((0, 2)), post((6, 8))]).expect("two blocks");
        let drawn_posts = blocks_silhouette(&posts_alone);
        let drawn_lintel = blocks_silhouette(&Blocks::new(&[lintel]).expect("one block"));
        for column in 0..44u16 {
            for row in 0..drawn
                .height()
                .max(drawn_posts.height())
                .max(drawn_lintel.height())
            {
                let union = drawn_at(&drawn_posts, column, row) || drawn_at(&drawn_lintel, column, row);
                assert_eq!(drawn_at(&drawn, column, row), union, "column {column}, row {row}",);
            }
        }
    }

    /// **The defect this shape was written for.** A box's base is two 45° runs
    /// meeting at the tile's south corner, which is pixel for pixel what two walls
    /// meeting at a corner leave — so the wall detector reads a solid as a corner
    /// of a house and every pixel of its lid is lit as a vertical face.
    ///
    /// Asserted rather than described because it is the fixture the fix is
    /// measured against: what has to change is not this verdict but that
    /// something asks a different question first. See `docs/lighting.md`'s
    /// backlog, "found on a staircase in Britain".
    #[test]
    fn the_wall_detector_reads_a_solid_as_a_corner_of_a_house() {
        for height in [5u8, 10, 20] {
            assert_eq!(
                facing_of(&prism_silhouette(&Prism::box_of(height))),
                Some(Facing::Corner {
                    right: Face::East,
                    left: Face::South
                }),
                "a box {height} tall",
            );
        }
    }

    /// A stair's treads are flats in the picture, and they climb towards the side
    /// the prism says is up.
    ///
    /// The property that separates a stair from a box without depending on how
    /// many treads a particular graphic has: sample the profile along the climb
    /// and it never falls.
    #[test]
    fn a_stair_climbs_towards_its_high_side() {
        let stair = Prism::new(Face::East, &[2, 4, 6]).expect("three treads is a legal profile");
        assert_eq!(stair.height_at(0.0), 2);
        assert_eq!(stair.height_at(0.5), 4);
        assert_eq!(stair.height_at(1.0), 6);
        assert_eq!(stair.top(), 6);

        // And the drawing says the same thing, read where the diamond has no
        // thickness to add: the picture's right end is the tile's east corner and
        // its left end the west one, so those two columns are the two ends of the
        // climb and nothing else.
        let picture = prism_silhouette(&stair);
        let profile = profile(&picture);
        let side = |column: usize| {
            let (base, top) = profile[column].expect("a drawn column");
            (i32::from(top) - i32::from(base)) / Z_STEP as i32
        };
        assert_eq!(side(0), 2, "the west corner stands on the low tread");
        assert_eq!(side(43), 6, "the east corner stands on the high one");
    }

    /// The render-side twin of `occlusion.rs`'s
    /// `a_stair_is_two_faces_per_tread_and_each_ones_height_comes_off_the_art`,
    /// same fixture: `0x0736`'s three treads, one to five `z`, climbing west.
    /// `Prism::mesh` and `Builder::add`'s climbable branch read the same two
    /// facts (`treads`, `up`), so the two tests pin the same shape from the
    /// two sides `docs/gbuffer.md` step 4c joins.
    #[test]
    fn a_stairs_mesh_is_two_honest_faces_per_tread() {
        let prism = Prism::new(Face::West, &[1, 3, 5]).expect("three treads");
        let mesh = prism.mesh(100, 100, 0);
        let faces = mesh.faces();
        assert_eq!(faces.len(), 6, "a top and a riser per tread, not one body");

        // `Prism::mesh` pushes a top then its riser, per tread, in climb order.
        let heights = [1i32, 3, 5];
        for (tread, &height) in heights.iter().enumerate() {
            let top = &faces[tread * 2];
            assert_eq!(
                top.normal,
                [0.0, 0.0, 1.0],
                "a top is flat, not blended towards the climb"
            );
            assert!(
                top.vertices().iter().all(|v| v.z == f64::from(height)),
                "tread {tread}'s own height, not the flight's tallest"
            );

            let riser = &faces[tread * 2 + 1];
            assert_eq!(
                riser.normal,
                [1.0, 0.0, 0.0],
                "a riser faces away from `up` (West), which is East's own outward"
            );
            let low = if tread == 0 {
                0.0
            } else {
                f64::from(heights[tread - 1])
            };
            // Exactly `low`/`height`, both ends. A riser used to be grown a
            // hairline past each — see the comment where `SEAM_OVERLAP` stood.
            let zs: Vec<f64> = riser.vertices().iter().map(|v| v.z).collect();
            assert!(
                zs.contains(&low),
                "riser {tread} should start at exactly {low}: {zs:?}"
            );
            assert!(
                zs.contains(&f64::from(height)),
                "riser {tread} should stop at exactly {height}: {zs:?}"
            );
        }
    }

    /// **The seam is closed by construction, and this is the construction.**
    ///
    /// A tread's top and its own riser share an edge, and every corner of it is
    /// bit-identical on both sides — the same `f64`, not two values a tolerance
    /// would call equal. That is the whole reason the retired `SEAM_OVERLAP` is
    /// not needed: [`crate::statics::push_mesh`] projects a corner with a pure
    /// function of that corner, so identical corners land on identical screen
    /// positions and the rasteriser's own fill rule gives every pixel of the
    /// shared edge to exactly one of the two triangles. `examples/synthetic_stair`
    /// measured the consequence — zero pixels inside a flight's silhouette
    /// belonging to no face, over thirty-six renders — and this is the property
    /// that measurement rests on, stated where it can fail loudly.
    ///
    /// Both edges of every riser: the one it shares with its own tread's top, and
    /// the one it shares with the tread below it. Every climb direction, because
    /// [`Prism::footprint`]'s four arms are four separate expressions and only
    /// this says they agree with themselves.
    #[test]
    fn a_tread_and_its_riser_share_an_edge_bit_for_bit() {
        for up in [Face::North, Face::East, Face::South, Face::West] {
            let heights = [1u8, 3, 5];
            let prism = Prism::new(up, &heights).expect("three treads");
            let mesh = prism.mesh(100, 100, 0);
            let faces = mesh.faces();
            // The corners of one face at a given `z`, sorted, so two rings that
            // list the same edge in different orders compare equal.
            let edge_at = |face: &crate::mesh::Face, z: f64| {
                let mut corners: Vec<(u64, u64)> = face
                    .vertices()
                    .iter()
                    .filter(|corner| corner.z == z)
                    .map(|corner| (corner.x.to_bits(), corner.y.to_bits()))
                    .collect();
                corners.sort_unstable();
                corners
            };
            for (tread, &height) in heights.iter().enumerate() {
                let top = &faces[tread * 2];
                let riser = &faces[tread * 2 + 1];
                let z = f64::from(height);
                let shared = edge_at(riser, z);
                assert_eq!(shared.len(), 2, "{up:?} riser {tread} has a top edge");
                let over = edge_at(top, z);
                assert!(
                    shared.iter().all(|corner| over.contains(corner)),
                    "{up:?}: riser {tread}'s top edge is not two corners of its own tread's top"
                );
                if tread == 0 {
                    continue;
                }
                // And downwards: the riser's low edge lies in the plane of the
                // tread below, at that tread's own height.
                let below = f64::from(heights[tread - 1]);
                let low = edge_at(riser, below);
                assert_eq!(low.len(), 2, "{up:?} riser {tread} has a bottom edge");
                let under = edge_at(&faces[(tread - 1) * 2], below);
                assert!(
                    low.iter().all(|corner| under.contains(corner)),
                    "{up:?}: riser {tread}'s bottom edge is not two corners of the tread below it"
                );
            }
        }
    }

    /// [`WIDTH_OVERLAP`]'s own doc: every face [`Prism::mesh`] builds is grown
    /// a hairline past the tile-crossing edge [`Prism::footprint`] holds at
    /// the unit square regardless of `lo`/`hi`, so a picture whose true art
    /// silhouette overruns the fitted box by less than a pixel still has
    /// something drawn under it. West's climb axis is `x` (this test's own
    /// fixture below pins that), so the grown edge is `y`.
    #[test]
    fn a_treads_top_is_grown_past_its_own_tile_edge() {
        let prism = Prism::new(Face::West, &[1, 3, 5]).expect("three treads");
        let mesh = prism.mesh(100, 100, 0);
        let top = &mesh.faces()[0];
        let ys: Vec<f64> = top.vertices().iter().map(|v| v.y).collect();
        assert!(
            ys.contains(&(100.0 - WIDTH_OVERLAP)),
            "the near edge should overrun the tile by WIDTH_OVERLAP: {ys:?}"
        );
        assert!(
            ys.contains(&(101.0 + WIDTH_OVERLAP)),
            "the far edge should overrun the tile by WIDTH_OVERLAP: {ys:?}"
        );
    }

    /// [`Prism::footprint`] pinned at the same fixture
    /// `occlusion.rs`'s `a_mid_flight_risers_footprint_stays_on_its_own_tile`
    /// uses — that test cannot reach this function directly (private to its
    /// module), but both now call it, so a regression here shows there too.
    #[test]
    fn a_mid_flight_treads_footprint_is_a_fraction_not_a_tile_edge() {
        let (min_x, max_x, min_y, max_y) = Prism::footprint(100.0, 100.0, Face::West, 1.0 / 3.0, 1.0 / 3.0);
        assert_eq!((min_x, max_x), (100.0 + 2.0 / 3.0, 100.0 + 2.0 / 3.0));
        assert_eq!(
            (min_y, max_y),
            (100.0, 101.0),
            "West's climb axis is x, so y stays the whole tile"
        );
    }
}
