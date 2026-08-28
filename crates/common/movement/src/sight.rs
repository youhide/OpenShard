//! The ray a look travels along, and what stops it.
//!
//! [`sight_clear`](crate::sight_clear) answers `true` or `false`, which is the
//! whole of what a shot needs and none of what a person debugging one needs: a
//! refusal names no tile, no graphic and no height. This module is that same
//! walk with its work written down — one [`SightStep`] per tile crossed, each
//! carrying the height the ray was at there and, where the ray ended, the
//! [`Stop`] that ended it.
//!
//! **The trace is the rule; the boolean is a reading of it.** `sight_clear` is
//! `trace(.., Extent::ToFirstBlock).clear()` and nothing else, so there is one
//! loop, one eye height and one set of thresholds. A diagnostic that walked its
//! own copy of the line would answer about that copy — `docs/parity.md`'s
//! standing complaint, and worse here than there, because a sight overlay drawn
//! from the wrong walk looks exactly like one drawn from the right walk.
//!
//! The two layers it reads, in the order it reads them, are documented on
//! [`MapTerrain::sight_stop`](crate::MapTerrain::sight_stop) (the map: ground,
//! walls, platforms, the window hole) and on [`trace`] itself (the live world's
//! shut doors). `docs/sight.md` is the plan this came from and carries the list
//! of properties the picture exposes.

use openshard_map::grid::Tile;
use openshard_protocol::wire::Graphic;
use openshard_protocol::world::Point;

use crate::footing::Footing;
use crate::walk::line_tiles;

/// How far above the ground the ray runs.
///
/// Head height, and everybody's: a dragon, a rabbit and a mounted knight all
/// look from nine units above their own `z`. That is the rule as it stands
/// rather than a rule this module chose — see `docs/sight.md`'s closing list.
pub const EYE: i32 = 9;

/// Why a look stopped at one tile.
///
/// Copy rather than borrowed: a stop outlives the map read that produced it —
/// it is drawn a frame later, and printed into a message after that.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Stop {
    /// The land itself rose above the eye line. A hill, in one word.
    Ground {
        /// The ground height that was above the ray.
        z: i32,
    },
    /// A static, with the span that stopped the ray.
    Static {
        /// The art in the way, which is what names it to a person.
        graphic: Graphic,
        /// Its base `z`.
        base: i32,
        /// One past the top of the span that stopped the ray — **not always the
        /// art's own height**, which is what `wallish` is here to say.
        top: i32,
        /// Which of the two readings gave it that span: a wall lent a whole
        /// storey (tiledata gives walls height 0 and the client draws them a
        /// storey tall), or a platform kept its real height.
        wallish: bool,
    },
    /// A shut door the live world put there.
    ///
    /// It carries no span, because the question asked of the live layer has no
    /// height in it: `Overlay::blocker_anywhere` is asked about a tile, and a
    /// shut door is opaque at every height of it. See `docs/sight.md`.
    Door,
    /// A structural wall the live world put there, such as a house component.
    LiveWall {
        /// Its base `z`.
        base: i32,
        /// One past the wall's sight span.
        top: i32,
    },
}

/// One tile of the line, and the ray's height over it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SightStep {
    /// The tile crossed.
    pub tile: Tile,
    /// How high the ray was here — the interpolated eye line, not the ground.
    pub ray_z: i32,
    /// What stopped the ray here, if anything did.
    pub stop: Option<Stop>,
}

/// How much of the line a caller wants walked.
///
/// A shot and a picture want different walks, which is why this is a parameter
/// rather than a policy: a shot is over at the first blocker, and a picture
/// that stopped there would end in mid-air with nothing to say which end the
/// archer is standing at.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Extent {
    /// Stop at the first blocker and record nothing. What a shot asks — combat
    /// runs it per tick per running action and aggro runs it per creature per
    /// tick, so it allocates only the tile list it walks.
    ToFirstBlock,
    /// Cross the whole line, recording every tile including the ones behind the
    /// blocker. What a picture asks.
    WholeLine,
}

/// A look from one point to another, and everything it met.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SightTrace {
    /// Where the look started.
    pub from: Point,
    /// What it was aimed at.
    pub to: Point,
    /// Every tile crossed, in order — **empty** under
    /// [`Extent::ToFirstBlock`], which does not record what it does not need.
    pub steps: Vec<SightStep>,
    /// Where the ray first stopped, if it did. Filled under either extent: it
    /// is the verdict, and [`clear`](Self::clear) is a reading of it.
    pub stopped: Option<SightStep>,
}

impl SightTrace {
    /// Whether the look got through.
    #[must_use]
    pub const fn clear(&self) -> bool {
        self.stopped.is_none()
    }
}

/// Walk the sight line from `from` to `to`.
///
/// # The line
///
/// Bresenham's, [`line_tiles`], and **both endpoints are excluded** — an archer
/// and their quarry do not stand in their own way.
///
/// # The height
///
/// The ray runs at [`EYE`] over a `z` interpolated between the two ends by tile
/// *index*, so a look up a hill follows the slope. By index and not by
/// distance: on a diagonal that puts the ray slightly off where the straight
/// line in world space is, which is the rule as it stands.
///
/// # The two layers
///
/// The map first — [`MapTerrain::sight_stop`](crate::MapTerrain::sight_stop) —
/// and then the live world, which contributes shut doors and structural house
/// walls. A crate is furniture, not a wall: `Overlay::sight_blocker_at` leaves
/// ordinary live movement blockers out of the ray.
#[must_use]
pub fn trace(footing: &Footing<'_>, from: Point, to: Point, extent: Extent) -> SightTrace {
    let tiles = line_tiles(Tile::new(from.x, from.y), Tile::new(to.x, to.y));
    let count = tiles.len() as i32;
    let mut steps = Vec::new();
    let mut stopped = None;
    for (index, tile) in tiles.into_iter().enumerate() {
        let ray_z = ray_height(i32::from(from.z), i32::from(to.z), index as i32, count);
        let stop = footing
            .map
            .and_then(|map| map.sight_stop(tile, ray_z))
            .or_else(|| live_stop(footing, tile, ray_z));
        let step = SightStep { tile, ray_z, stop };
        if stop.is_some() && stopped.is_none() {
            stopped = Some(step);
        }
        match extent {
            Extent::ToFirstBlock => {
                if stopped.is_some() {
                    break;
                }
            }
            Extent::WholeLine => steps.push(step),
        }
    }
    SightTrace {
        from,
        to,
        steps,
        stopped,
    }
}

/// A live-world obstruction on `tile`, if it stops the ray.
fn live_stop(footing: &Footing<'_>, tile: Tile, ray_z: i32) -> Option<Stop> {
    footing
        .overlay
        .sight_blocker_at(tile, ray_z)
        .map(|cover| match cover.is_door() {
            true => Stop::Door,
            false => Stop::LiveWall {
                base: cover.bottom(),
                top: cover.sight_top(),
            },
        })
}

/// How high the ray is over the `index`th tile of a line `count` tiles long.
///
/// The two ends' own heights, interpolated, plus the eye. `count + 1` is the
/// denominator and `index + 1` the numerator because the endpoints are not in
/// the list: the first recorded tile is already one step along the line, and
/// the last one is one step short of the target.
fn ray_height(from_z: i32, to_z: i32, index: i32, count: i32) -> i32 {
    let t = index + 1;
    from_z + (to_z - from_z) * t / (count + 1) + EYE
}

#[cfg(test)]
mod tests {
    use openshard_map::overlay::{Cover, Doors, Overlay};
    use openshard_tiles::TileFlags;

    use super::*;
    use crate::scene::Scene;

    /// A look across a flat scene from `(1, 4)` to `(6, 4)`, whole line.
    fn look_across(scene: &Scene) -> SightTrace {
        trace(
            &scene.footing(),
            Point::new(1, 4, 0),
            Point::new(6, 4, 0),
            Extent::WholeLine,
        )
    }

    /// A window is the deliberate hole: the same art with and without the bit
    /// is the same wall, and only one of them stops a look.
    #[test]
    fn a_window_is_a_hole_in_a_wall_a_look_passes_through() {
        let mut opaque = Scene::flat(0);
        opaque.art(0x400, TileFlags::WALL, 20).put(3, 4, 0, 0x400);
        assert!(!look_across(&opaque).clear(), "the wall did not stop a look");

        let mut glazed = Scene::flat(0);
        glazed
            .art(0x401, TileFlags::WALL | TileFlags::WINDOW, 20)
            .put(3, 4, 0, 0x401);
        assert!(look_across(&glazed).clear(), "a look did not cross a window");
    }

    /// The storey a wall is lent, and the storey a platform is not.
    ///
    /// Both are height 0 in the table. The wall is drawn a storey tall and
    /// stops the ray; the floor tile is flat and stops nothing — which is what
    /// keeps an open doorway a sight line, since a doorway's floor is laid in
    /// the very tile the look crosses.
    #[test]
    fn a_wall_of_no_height_is_lent_a_storey_and_a_flat_platform_is_not() {
        let mut walled = Scene::flat(0);
        walled.art(0x402, TileFlags::WALL, 0).put(3, 4, 0, 0x402);
        let stopped = look_across(&walled).stopped.expect("the wall stops the look");
        assert_eq!(
            stopped.stop,
            Some(Stop::Static {
                graphic: openshard_protocol::wire::Graphic(0x402),
                base: 0,
                top: 15,
                wallish: true,
            }),
            "a zero-height wall was not lent its storey"
        );

        let mut floored = Scene::flat(0);
        floored.art(0x403, TileFlags::PLATFORM, 0).put(3, 4, 0, 0x403);
        assert!(
            look_across(&floored).clear(),
            "a flat floor tile walled off the tile it is laid in"
        );
    }

    /// An upper floor is what stops you seeing the storey above you: a platform
    /// whose real span the eye line falls inside.
    #[test]
    fn a_platform_stops_the_ray_that_runs_inside_its_own_span() {
        let mut scene = Scene::flat(0);
        // The ray runs at `EYE` over flat ground; a ceiling based just under it
        // and one unit thick is exactly what it meets.
        scene
            .art(0x404, TileFlags::PLATFORM, 1)
            .put(3, 4, EYE as i8, 0x404);
        let stopped = look_across(&scene).stopped.expect("the ceiling stops the look");
        assert_eq!(
            stopped.stop,
            Some(Stop::Static {
                graphic: openshard_protocol::wire::Graphic(0x404),
                base: EYE,
                top: EYE + 1,
                wallish: false,
            })
        );
    }

    /// A hill in the way, named as one rather than as a static.
    #[test]
    fn ground_above_the_eye_line_is_the_stop() {
        let mut scene = Scene::flat(0);
        // A whole hummock rather than one raised corner: `ground_z` is the
        // *average* of a tile's four corners, the way the client draws it, so
        // one lifted corner is a quarter of a hill.
        for x in 3..=4 {
            for y in 3..=5 {
                scene.ground(x, y, 40);
            }
        }
        // Where it stops is the near *slope* rather than the flat top: a tile
        // west of the raised ones shares two corners with them and so already
        // averages high.
        let stopped = look_across(&scene).stopped.expect("the hill stops the look");
        assert!(
            matches!(stopped.stop, Some(Stop::Ground { z }) if z > stopped.ray_z),
            "the hill was not the stop, or was not above the ray"
        );
    }

    /// The eye is added to an interpolation that has both ends' heights in it.
    #[test]
    fn the_ray_climbs_between_the_two_ends() {
        // Four tiles between ends twenty apart: the ray rises by four a tile
        // (20 / 5) and carries the eye the whole way.
        assert_eq!(ray_height(0, 20, 0, 4), 4 + EYE);
        assert_eq!(ray_height(0, 20, 3, 4), 16 + EYE);
        // A level look is the eye and nothing else, wherever along it you ask.
        assert_eq!(ray_height(7, 7, 2, 9), 7 + EYE);
    }

    /// With no map at all, the live world is the only thing that can be in the
    /// way: shut doors and walls, but not furniture.
    #[test]
    fn a_shut_door_and_a_live_wall_stop_but_a_crate_does_not() {
        let mut live = Overlay::default();
        live.set(Tile::new(10, 10), vec![Cover::door(0, 20)]);
        let footing = Footing::new(None, &live, Doors::AsTheyStand);
        let looked = trace(
            &footing,
            Point::new(10, 8, 0),
            Point::new(10, 12, 0),
            Extent::WholeLine,
        );
        assert_eq!(
            looked.stopped.map(|step| step.stop),
            Some(Some(Stop::Door)),
            "the shut door did not stop the look"
        );
        assert!(!looked.clear());

        let mut live = Overlay::default();
        live.set(Tile::new(10, 10), vec![Cover::blocking(0, 0).as_sight_wall()]);
        let footing = Footing::new(None, &live, Doors::AsTheyStand);
        let looked = trace(
            &footing,
            Point::new(10, 8, 0),
            Point::new(10, 12, 0),
            Extent::WholeLine,
        );
        assert_eq!(
            looked.stopped.map(|step| step.stop),
            Some(Some(Stop::LiveWall { base: 0, top: 15 })),
            "a zero-height live wall was not lent its storey"
        );

        let mut live = Overlay::default();
        live.set(Tile::new(10, 10), vec![Cover::blocking(0, 5).as_sight_blocker()]);
        let footing = Footing::new(None, &live, Doors::AsTheyStand);
        assert!(
            trace(
                &footing,
                Point::new(10, 8, 0),
                Point::new(10, 12, 0),
                Extent::WholeLine,
            )
            .clear(),
            "a low platform wall walled off a ray above its real span"
        );

        let mut live = Overlay::default();
        live.set(Tile::new(10, 10), vec![Cover::blocking(0, 20)]);
        let footing = Footing::new(None, &live, Doors::AsTheyStand);
        assert!(
            trace(
                &footing,
                Point::new(10, 8, 0),
                Point::new(10, 12, 0),
                Extent::WholeLine,
            )
            .clear(),
            "a crate is furniture, not a wall"
        );
    }

    /// Neither end is walked: an archer and their quarry are not in their own
    /// way, which is `line_tiles`' rule and this is the check that the trace
    /// inherits it.
    #[test]
    fn the_two_ends_are_not_steps_of_the_line() {
        let mut live = Overlay::default();
        live.set(Tile::new(10, 8), vec![Cover::door(0, 20)]);
        live.set(Tile::new(10, 12), vec![Cover::door(0, 20)]);
        let footing = Footing::new(None, &live, Doors::AsTheyStand);
        let looked = trace(
            &footing,
            Point::new(10, 8, 0),
            Point::new(10, 12, 0),
            Extent::WholeLine,
        );
        assert!(looked.clear(), "an endpoint blocked its own look");
        assert_eq!(looked.steps.len(), 3, "the three tiles between the ends");
    }

    /// The two extents answer the same verdict and record differently.
    #[test]
    fn the_whole_line_carries_on_past_the_first_stop() {
        let mut live = Overlay::default();
        live.set(Tile::new(10, 10), vec![Cover::door(0, 20)]);
        live.set(Tile::new(10, 11), vec![Cover::door(0, 20)]);
        let footing = Footing::new(None, &live, Doors::AsTheyStand);
        let from = Point::new(10, 8, 0);
        let to = Point::new(10, 13, 0);

        let whole = trace(&footing, from, to, Extent::WholeLine);
        assert_eq!(whole.steps.len(), 4, "(10,9) through (10,12)");
        assert_eq!(
            whole.steps.iter().filter(|step| step.stop.is_some()).count(),
            2,
            "the second door is behind the first and still recorded"
        );
        assert_eq!(
            whole.stopped.map(|step| step.tile),
            Some(Tile::new(10, 10)),
            "the verdict is the *first* stop"
        );

        let first = trace(&footing, from, to, Extent::ToFirstBlock);
        assert!(first.steps.is_empty(), "a shot records nothing");
        assert_eq!(
            first.stopped.map(|step| step.tile),
            whole.stopped.map(|step| step.tile),
            "the two extents disagreed about the verdict"
        );
    }
}
