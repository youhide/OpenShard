//! A box in world coordinates, and the three faces of it a camera can see.
//!
//! `docs/lighting.md` decision 39: the scene this client draws is already
//! three-dimensional — world space, a per-pixel world position, an orthographic
//! projection and a hardware depth buffer are all in place — and what is missing
//! is a primitive that is not a billboard. This is that primitive's geometry,
//! and it is deliberately only the geometry: a [`Solid`] knows where it stands
//! and hands back polygons, and whether those polygons are painted by egui over
//! the frame or by an instanced quad pass is the caller's business.
//!
//! **Three faces, always the same three.** There is no rotation anywhere in this
//! renderer, so an axis-aligned box always shows `+x`, `+y` and its top, its
//! outline is a hexagon, and the three faces tile that hexagon without
//! overlapping. That is what makes this a handful of arithmetic rather than a
//! mesh pipeline: no index buffer, no back-face culling, nothing to cull against.

use crate::camera::{Camera, WorldSpot, project_exact};
use crate::geometry::Vec2;

/// A box standing in the world: two opposite corners in the world's own units.
///
/// `min` is the low corner on all three axes and `max` the high one, in
/// [`WorldSpot`]'s corner lattice — so the solid that fills tile `(x, y)` from
/// the ground to a height of 20 is `min = (x, y, 0)`, `max = (x+1, y+1, 20)`.
/// Nothing here clips a solid to a tile, and that is decision 38: a solid is a
/// shape the world holds, and the tile grid only *finds* it.
///
/// A degenerate extent is allowed and means what it says — a `min.z == max.z`
/// box is the plane a lid is today, and it draws as a flat diamond rather than
/// as an error. What the geometry cannot express is a rotation, and it never
/// will: see the module doc.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Solid {
    /// The low corner: west, north and bottom.
    pub min: WorldSpot,
    /// The high corner: east, south and top.
    pub max: WorldSpot,
}

/// Which of a solid's three visible faces a polygon is.
///
/// The two vertical ones are [`facing::Face::East`](crate::facing::Face) and
/// `South` — the same two edges of a tile the art draws, for the same reason:
/// this camera looks from the north-west, so `+x` and `+y` are what is turned
/// towards it. The top has no edge and so no `Face`, which is why this is its
/// own small enum rather than a reuse.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Side {
    /// The lid, at `max.z`. The face that looks at the sky, and the one that
    /// makes a solid's thickness legible: a wall with no top face drawn is
    /// indistinguishable from the plane it used to be.
    Top,
    /// The `x = max.x` face, on the lower-right of the hexagon.
    East,
    /// The `y = max.y` face, on the lower-left.
    South,
}

impl Side {
    /// The lid, full strength — it is the face that looks at the light.
    pub const TOP_SHADE: f32 = 1.0;
    /// The `+x` face.
    pub const EAST_SHADE: f32 = 0.78;
    /// The `+y` face, a step darker again.
    pub const SOUTH_SHADE: f32 = 0.56;

    /// How much of its colour this face keeps.
    ///
    /// The fixed light this view is lit by, from above and to the east — not a
    /// simulation of anything, and deliberately not the world's own sun, which
    /// would leave the instrument unreadable at dusk, exactly when the picture
    /// is hardest to read. What it is for is that two faces meeting at one line
    /// come out as two tones, so an edge is visible without a stroke having to
    /// say where it is.
    ///
    /// One table, read by the solids pass and by the wireframe beside it: two
    /// views shading the same box differently would be two claims about which
    /// side of it is which.
    pub fn shade(self) -> f32 {
        match self {
            Self::Top => Self::TOP_SHADE,
            Self::East => Self::EAST_SHADE,
            Self::South => Self::SOUTH_SHADE,
        }
    }
}

impl Solid {
    /// The three faces, in viewport pixels, each as four corners in ring order.
    ///
    /// One projection for all twelve points and it is
    /// [`project_exact`] — the same arithmetic every sprite in the frame is
    /// placed by, so a solid drawn round a wall lands on the wall's own pixels
    /// with nothing fitted. The corners run through the float path end to end
    /// ([`Camera::to_viewport_exact`]): a face a fifth of a tile deep has ends
    /// between the virtual pixels, and rounding each corner on its own would
    /// bend a plane.
    ///
    /// The three polygons tile the solid's hexagonal outline and never overlap,
    /// so a caller painting them translucent gets one weight of colour per pixel
    /// and may draw them in any order.
    pub fn faces(&self, camera: &Camera) -> [(Side, [Vec2; 4]); 3] {
        let at = |x: f64, y: f64, z: f64| {
            camera.to_viewport_exact(camera.to_view_exact(project_exact(WorldSpot { x, y, z })))
        };
        let (lo, hi) = (self.min, self.max);
        [
            // The diamond, in `Camera::tile_facet`'s own order — north corner,
            // east, south, west — so a unit solid's top face and the tile
            // diamond under it are the same four points. The test beside this
            // asserts exactly that.
            (
                Side::Top,
                [
                    at(lo.x, lo.y, hi.z),
                    at(hi.x, lo.y, hi.z),
                    at(hi.x, hi.y, hi.z),
                    at(lo.x, hi.y, hi.z),
                ],
            ),
            // Both vertical faces run top edge first and then down, so the pair
            // of corners a face shares with the top is its first two — which is
            // what a caller wants when it strokes only the silhouette.
            (
                Side::East,
                [
                    at(hi.x, lo.y, hi.z),
                    at(hi.x, hi.y, hi.z),
                    at(hi.x, hi.y, lo.z),
                    at(hi.x, lo.y, lo.z),
                ],
            ),
            (
                Side::South,
                [
                    at(lo.x, hi.y, hi.z),
                    at(hi.x, hi.y, hi.z),
                    at(hi.x, hi.y, lo.z),
                    at(lo.x, hi.y, lo.z),
                ],
            ),
        ]
    }

    /// The lines that make a box read as a box: its silhouette, and the three
    /// edges inside it where the faces meet.
    ///
    /// Nine, and **not** the four edges of each face, which is twelve with three
    /// drawn twice. The difference is not tidiness: a run of wall stroked
    /// per-face is a lattice of doubled lines, and a tile carrying several
    /// surfaces goes black. The shape a stroke belongs on is the outline of the
    /// solid — the hexagon of decision 39.3 — plus the interior star that says
    /// which of the three faces is which.
    ///
    /// Derived from [`Solid::faces`] rather than projected again, so a change to
    /// the corner order moves the strokes with the fills.
    pub fn outline(&self, camera: &Camera) -> [[Vec2; 2]; 9] {
        let faces = self.faces(camera);
        let (top, east, south) = (faces[0].1, faces[1].1, faces[2].1);
        // The hexagon, walked round: over the top's north and east corners, down
        // the east face's far edge, along the bottom, and back up the south's.
        [
            [top[0], top[1]],
            [top[1], east[3]],
            [east[3], east[2]],
            [east[2], south[3]],
            [south[3], top[3]],
            [top[3], top[0]],
            // And the star: the two edges the top shares with the faces under
            // it, and the vertical where those two meet.
            [top[1], top[2]],
            [top[3], top[2]],
            [top[2], east[2]],
        ]
    }
}

/// How thick a lid is drawn, in `z` units.
///
/// Two, which is 8 pixels of visible side band at 1:1. Chosen to be *seen* —
/// the art cannot measure a wall's depth (decision 3) and a lid stays flat
/// where a panel no longer does (see
/// [`occlusion::PANEL_THICKNESS`](crate::occlusion::PANEL_THICKNESS)), so this
/// is still a drawing number rather than one a ray is tested against.
///
/// **`DRAWN_` is the whole of the name and it is a fence.** A panel had one of
/// these too, once — before step 23.5 gave `Solid::box_of` a real slab, the
/// view was the only reader thick enough to find a wall the eye could not.
/// Now the grid's own box already has the depth, [`drawn`] draws a panel
/// exactly as it stands, and the fence only ever guarded the one kind that
/// still needs it.
pub const DRAWN_LID_THICKNESS: f64 = 2.0;

/// The box a view draws for one solid: its own, with whatever it is flat on
/// given a thickness to be seen by.
///
/// **A drawing, not a measurement — and since step 23.5, almost the identity.**
/// The grid's box is the geometry the walk is tested against, and since
/// `Solid::box_of` gave a panel a real slab, that box is already what a person
/// needs to find it: only a lid's height is still fattened, downwards by
/// [`DRAWN_LID_THICKNESS`], so a floor and the tile under it stop telling the
/// same story. A body and a panel are already boxes and pass through untouched.
///
/// The `z` is clamped into an `i8` the way the upload clamps it
/// ([`Occlusion::solid_bytes`](crate::occlusion::Occlusion::solid_bytes)), so
/// that the box is drawn where the **shader** believes it is rather than where
/// the map says. That is the whole point of a view of the grid, and it is why
/// the clamp is not the caller's to remember.
pub fn drawn(solid: &crate::occlusion::Solid) -> Solid {
    let space = solid.space;
    let height = |z: f64| z.clamp(f64::from(i8::MIN), f64::from(i8::MAX));
    let (mut min, mut max) = (space.min, space.max);
    min.z = height(min.z);
    max.z = height(max.z);
    if solid.edges == 0 {
        // A lid — a floor, a roof, a plank. A slab hanging under the height it
        // lies at: the surface a ray is stopped by is the top one, so the
        // thickness goes downwards.
        //
        // The bottom is *replaced* and not lowered to reach, which is what step
        // 23.0 drew and is kept here to the pixel. It means a lid with a span of
        // its own — a static with a `FLOOR` bit and a height — is drawn two `z`
        // deep rather than as deep as it stops light over, and that is a real
        // thing to look at rather than a rounding: `docs/lighting.md`'s backlog
        // carries it, because changing it is a picture moving and step 23.1's
        // whole claim is that no picture moved.
        min.z = max.z - DRAWN_LID_THICKNESS;
    }
    Solid { min, max }
}

/// What colour a solid is drawn in — **the hue is the kind**.
///
/// It was the opacity first, and that is the mistake worth keeping written
/// down: opacity is nearly a constant in a real town (5,459 `OPAQUE` against 74
/// panes over the block this view was built on), so the picture came out one
/// flat red and the geometry, which is the entire question, had no colour left
/// to be told in. A pane is rare enough to be the exception rather than the
/// axis.
///
/// Here rather than in either view because both read it: the solids pass and the
/// wireframe are looked at against each other — a red plane standing inside an
/// amber box is a defect, and it is invisible if the two chose their own reds.
pub fn kind_colour(solid: &crate::occlusion::Solid) -> [f32; 3] {
    use crate::occlusion::{EDGE_ANY, OPAQUE};

    match (solid.opacity < OPAQUE, solid.edges) {
        // A pane, whatever shape it is: the one thing here that is about how
        // much light gets through rather than about where a plane is.
        (true, _) => [60.0, 200.0, 255.0],
        // A lid — a floor, a roof, a plank. Warm, because it is the face that
        // looks at the light, and because its *absence* over a tile is what a
        // person opening this view is usually hunting.
        (false, 0) => [255.0, 190.0, 70.0],
        // A body: a graphic whose art named no edge, so the whole tile stops
        // light. Violet, and deliberately the odd colour out — it is a fallback
        // rather than a measurement, and a street with one of these standing
        // among panels is worth seeing from across the room.
        (false, EDGE_ANY) => [190.0, 90.0, 255.0],
        // A panel: a wall on one named edge.
        (false, _) => [255.0, 70.0, 60.0],
    }
}

/// How much of the grid a view of it draws — **the second datum**, and the
/// answer to the one thing the first version of this instrument could not say.
///
/// A view of the grid drew what stands above the player's feet, and that is
/// right for a picture of what shadows *you*: a pier is one thin slab on every
/// plank, and a frame of it was 2,011 boxes with the walls somewhere inside them
/// (see [`Solid::stands`](crate::occlusion::Solid::stands)). It is wrong for
/// a view whose subject is geometry. Standing in a room at `z = 0`, the room's
/// own floor and every lid under it are simply not drawn — and **a hole in a
/// floor and a floor below the cut are the same picture**, which is an
/// instrument that cannot be told from the defect it is pointed at.
///
/// Counting what was hidden does not close it, and that is worth being explicit
/// about: both views say how many they dropped, and a number says *how much* is
/// missing rather than *where*. So the datum is a switch a person throws, and
/// the two values are the two questions:
///
/// - [`Cut::BelowFeet`] — what could shadow somebody standing here.
/// - [`Cut::Nothing`] — what the grid holds. Unreadable in a town by design;
///   it is what a person switches to when the question is "is that floor
///   there at all".
///
/// **"This storey" is deliberately not a third value.** It is the one a person
/// would reach for most and it needs something nothing here has: a ceiling, and
/// therefore a rule for which lid over your head is *yours* when the grid holds
/// four of them. Inventing that rule to fill out an enum would put a third
/// answer in the instrument that no test could hold, which is how a diagnostic
/// starts lying. It arrives when a room is a thing the world can name.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Cut {
    /// Draw every solid the grid holds, wherever the eye is.
    Nothing,
    /// Drop everything at or below the feet of somebody standing at this `z`.
    BelowFeet(i8),
}

impl Cut {
    /// Whether a view under this cut draws `solid`.
    ///
    /// The one place the rule lives: two views read it, and a wireframe and a
    /// solids pass disagreeing about what is on screen would make the pair
    /// useless for exactly the comparison they exist for.
    pub fn shows(self, solid: &crate::occlusion::Solid) -> bool {
        match self {
            Self::Nothing => true,
            Self::BelowFeet(floor) => solid.stands(floor),
        }
    }
}

/// Every solid an occlusion grid holds that survives `cut`, coloured by kind and
/// ordered back to front.
///
/// The list both views draw, so that "what is on screen" is one answer. The
/// order is `depth::Order`'s own key — `x + y`, then height — and **this is
/// where decision 39.2 will bite**: one key per solid is right only while a
/// solid stands on one tile, and step 23.5's will not. Translucent and
/// depth-free, a wrong order costs a shade rather than a wrong picture, which is
/// exactly why that answer was taken first.
pub fn standing(occlusion: &crate::occlusion::Occlusion, cut: Cut) -> Vec<(Solid, [f32; 3])> {
    let bounds = occlusion.bounds();
    let mut found: Vec<(i32, i32, &crate::occlusion::Solid)> = (bounds.min_y..=bounds.max_y)
        .flat_map(|y| {
            (bounds.min_x..=bounds.max_x)
                .flat_map(move |x| occlusion.solids_at(x, y).map(move |solid| (x, y, solid)))
        })
        .filter(|(_, _, solid)| cut.shows(solid))
        .collect();
    // The height half of the key is the solid's own exact span and not
    // `bottom()`/`top()`'s rounded one: two boxes stacked half a unit apart tie
    // under a rounding, and a tie here is an arbitrary order between two things
    // one of which is genuinely in front of the other. `total_cmp` because the
    // key is `f32` and the sort wants a total order — a `z` here is never `NaN`
    // (it comes off a solid's own box), and `total_cmp` says so without a
    // comparator that would silently reorder if one ever were.
    found.sort_by(|(ax, ay, a), (bx, by, b)| {
        (ax + ay)
            .cmp(&(bx + by))
            .then(a.low().total_cmp(&b.low()))
            .then(a.high().total_cmp(&b.high()))
    });
    found
        .into_iter()
        .map(|(_, _, solid)| (drawn(solid), kind_colour(solid)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::Camera;
    use openshard_protocol::world::Point;

    fn camera() -> Camera {
        Camera::new(
            Point {
                x: 1500,
                y: 1600,
                z: 0,
            },
            800,
            600,
        )
    }

    /// The top of a solid that fills one tile is the tile's own diamond, to the
    /// float — which is the whole claim of decision 39.1: geometry placed in
    /// world coordinates lands in the same pixels the sprite for that tile
    /// lands in, with nothing fitted.
    ///
    /// This is also the test that catches the corner-versus-centre lattice
    /// getting confused, and it catches it as half a tile rather than as
    /// anything subtle.
    #[test]
    fn a_unit_solid_tops_its_tile_diamond() {
        let camera = camera();
        let point = Point {
            x: 1501,
            y: 1602,
            z: 7,
        };
        let solid = Solid {
            min: WorldSpot {
                x: f64::from(point.x),
                y: f64::from(point.y),
                z: 0.0,
            },
            max: WorldSpot {
                x: f64::from(point.x) + 1.0,
                y: f64::from(point.y) + 1.0,
                z: f64::from(point.z),
            },
        };
        let (side, top) = solid.faces(&camera)[0];
        assert_eq!(side, Side::Top);
        let diamond = camera.tile_diamond(point);
        for (got, want) in top.iter().zip(&diamond) {
            assert!(
                (got.x - want.x).abs() < 1e-3 && (got.y - want.y).abs() < 1e-3,
                "corner {got:?} against the tile's own {want:?}"
            );
        }
    }

    /// A solid one `z` tall is 4 pixels tall on the screen and 44 wide, not 44
    /// tall: the projection is anisotropic and the trap decision 39.1 names is
    /// authoring a "real cube" and getting something five and a half times too
    /// high. Pinned here so the number is in a test rather than in a comment.
    #[test]
    fn height_is_four_pixels_a_unit_and_the_ground_is_twenty_two() {
        let camera = camera();
        let flat = |z: f64| Solid {
            min: WorldSpot {
                x: 1500.0,
                y: 1600.0,
                z: 0.0,
            },
            max: WorldSpot {
                x: 1501.0,
                y: 1601.0,
                z,
            },
        };
        let north_of = |solid: Solid| solid.faces(&camera)[0].1[0];
        let ground = north_of(flat(0.0));
        let lifted = north_of(flat(1.0));
        assert!(
            (ground.y - lifted.y - 4.0).abs() < 1e-3,
            "one z should lift 4 pixels, lifted {}",
            ground.y - lifted.y
        );
        // The same solid's north corner against its east one: one tile of `+x`,
        // which is 22 across and 22 down and nothing like the 4 a `z` is.
        let corners = flat(0.0).faces(&camera)[0].1;
        assert!(
            (corners[1].x - corners[0].x - 22.0).abs() < 1e-3
                && (corners[1].y - corners[0].y - 22.0).abs() < 1e-3,
            "one tile east should move 22 across and 22 down, moved {:?}",
            (corners[1].x - corners[0].x, corners[1].y - corners[0].y)
        );
    }

    /// The two cuts differ by exactly the thing the datum exists for: the floor
    /// under your own feet.
    ///
    /// Decision 39.8. The fixture is the case the backlog entry was written
    /// about — a person standing on a floor at `z = 0` with a wall beside them —
    /// and under `BelowFeet` the floor is not drawn, which is right for a
    /// picture of what shadows them and is indistinguishable from a floor the
    /// grid failed to build. What this holds is that the other answer exists and
    /// is *strictly* the larger one: a cut that drew a different set rather than
    /// a superset would be a second opinion about the grid.
    #[test]
    fn the_whole_grid_is_the_grid_above_your_feet_plus_the_floor_you_stand_on() {
        use crate::camera::TileBounds;
        use crate::cutaway::Cutaway;
        use crate::occlusion::Builder;
        use openshard_protocol::wire::Graphic;
        use openshard_uofiles::tiledata::{StaticTile, TileFlags};

        let tile = |flags: u64, height: u8| StaticTile {
            flags: TileFlags::new(flags),
            height,
            ..StaticTile::default()
        };
        let mut grid = Builder::new(TileBounds {
            min_x: 100,
            max_x: 110,
            min_y: 100,
            max_y: 110,
        });
        // The plank underfoot: a floor is what you cannot shoot through, so it
        // is in the grid, and its top is exactly the height the body stands at.
        grid.add(
            104,
            104,
            0,
            Graphic(0),
            &tile(TileFlags::NO_SHOOT | TileFlags::FLOOR, 0),
            crate::occlusion::Shape::UNREAD,
        );
        // And a wall on the same tile, twenty tall.
        grid.add(
            104,
            104,
            0,
            Graphic(0),
            &tile(TileFlags::NO_SHOOT | TileFlags::WALL, 20),
            crate::occlusion::Shape::UNREAD,
        );
        let grid = grid.finish(&Cutaway::OPEN);

        let feet = standing(&grid, Cut::BelowFeet(0));
        let everything = standing(&grid, Cut::Nothing);
        assert_eq!(feet.len(), 1, "the floor under your feet is not what shadows you");
        assert_eq!(everything.len(), 2, "and the whole grid is where it can be seen");
        for solid in &feet {
            assert!(
                everything.contains(solid),
                "the cut should drop solids, not choose different ones",
            );
        }
    }

    /// The three faces meet at the solid's own centre-top corner and share their
    /// edges: the top's east corner is the east face's first, the top's south
    /// corner is the south face's first, and both verticals meet at the top's
    /// bottom corner. A box drawn from faces that do not share corners reads as
    /// three loose rhombi, which is the failure this catches.
    #[test]
    fn the_faces_share_their_edges() {
        let camera = camera();
        let solid = Solid {
            min: WorldSpot {
                x: 1500.0,
                y: 1600.0,
                z: 0.0,
            },
            max: WorldSpot {
                x: 1500.2,
                y: 1601.0,
                z: 20.0,
            },
        };
        let faces = solid.faces(&camera);
        let (top, east, south) = (faces[0].1, faces[1].1, faces[2].1);
        assert_eq!(top[1], east[0], "top's east corner is the east face's start");
        assert_eq!(top[2], east[1], "and they share the whole top edge");
        assert_eq!(top[3], south[0], "top's west corner is the south face's start");
        assert_eq!(top[2], south[1], "and the two verticals meet at the near corner");
    }
}
