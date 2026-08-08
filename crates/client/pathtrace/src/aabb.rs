//! An axis-aligned box, and where a ray crosses it.
//!
//! The slab method, written from the definition. It is a fourth implementation
//! of a ray-vs-box test in this workspace — `light::ray_vs_solid` on the CPU,
//! `ray_vs_solid` in `blit.wesl` on the GPU, `segment_clear_of_box` in the
//! render crate's own `examples/oracle` — and unlike the first three it is
//! deliberately *not* held to any of them. Holding four implementations of one
//! formula to each other is how a reference stops being one: the whole value of
//! the fourth is that nobody reconciled it with the others, so what it is held
//! to instead is a set of crossings worked out by hand, below.

use crate::vector::Vec3;

/// Where a ray meets a box, as two distances along it.
///
/// Distances and not points: a caller with several boxes wants to compare them
/// before it evaluates any, and the ray parameter is what it can compare. The
/// ray is a **line** and not a segment — `near` is routinely negative, for a
/// box behind the origin or one containing it — so a caller wanting a segment
/// does the clipping itself and is the one that decides what "behind me"
/// means. A line always has both ends, which is why neither axis below is
/// optional: even a line starting inside the box entered it, somewhere behind.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Crossing {
    /// Where the ray enters, in multiples of the direction vector.
    pub near: f64,
    /// And leaves. Never less than `near`; equal to it where the line meets the
    /// box at a single point — a corner, or a box with no extent on the axis
    /// the line crosses.
    pub far: f64,
    /// Which of the three slabs decided `near`, `0..3`.
    pub near_axis: usize,
    /// And which decided `far`. Not the same one in general, and taking the
    /// entry axis for both is a normal that points off a face the ray never
    /// touched.
    pub far_axis: usize,
}

/// An axis-aligned box: two opposite corners.
///
/// A box with no extent on some axis is an ordinary member here rather than a
/// degenerate case to reject. It is a plane, it is what a floor or a wall panel
/// is in the renderer's own occlusion grid, and the slab arithmetic answers for
/// it correctly: the two bounds of that axis coincide, so a ray crossing the
/// plane has `near == far` there and a ray *travelling inside* the plane has no
/// crossing at all. A plane is crossed, never travelled through.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Aabb {
    pub min: Vec3,
    pub max: Vec3,
}

impl Aabb {
    /// From two corners in any order.
    pub fn between(a: Vec3, b: Vec3) -> Self {
        Self {
            min: a.min(b),
            max: a.max(b),
        }
    }

    /// Where the line `from + t * direction` crosses this box, or [`None`] if it
    /// misses.
    ///
    /// `direction` need not be normalized; `near`/`far` come back in whatever
    /// unit it sets, which is what lets a caller pass `to - from` and read the
    /// answer as a fraction of a segment.
    ///
    /// **Grazing is a miss.** Where the line runs parallel to a pair of bounds
    /// and lies exactly *on* one of them — along a face, an edge or a lid — it
    /// touches the box without ever being inside it, and this returns [`None`].
    /// The alternative is an interval of real length that encloses no interior
    /// at all, which every caller would then have to recognise and discard
    /// again; and a shadow test that took it at face value would draw a hard
    /// dark line along every silhouette edge in the scene. A box with no extent
    /// on an axis — a plane — is the same rule with both bounds coincident: a
    /// plane is crossed, never travelled along.
    ///
    /// A line meeting the box at a single point — a corner, or a plane crossed
    /// at an angle — is a genuine crossing with `near == far`, and *is*
    /// reported. Whether zero length blocks light is the caller's decision.
    ///
    /// # Panics
    ///
    /// On a zero direction, which names no line at all.
    pub fn crossing(&self, from: Vec3, direction: Vec3) -> Option<Crossing> {
        assert!(
            direction != Vec3::ZERO,
            "a crossing of the zero direction: `from` and `to` are the same point"
        );
        let (mut near, mut far) = (f64::NEG_INFINITY, f64::INFINITY);
        let (mut near_axis, mut far_axis) = (None, None);
        for axis in 0..3 {
            let (origin, step) = (from.axis(axis), direction.axis(axis));
            let (lo, hi) = (self.min.axis(axis), self.max.axis(axis));
            // Parallel to this pair of bounds: the axis cannot decide where the
            // line enters, only whether it is strictly between them. Strictly —
            // see the note on grazing above.
            if step == 0.0 {
                if origin <= lo || origin >= hi {
                    return None;
                }
                continue;
            }
            let (mut enters, mut leaves) = ((lo - origin) / step, (hi - origin) / step);
            if enters > leaves {
                std::mem::swap(&mut enters, &mut leaves);
            }
            if enters > near {
                near = enters;
                near_axis = Some(axis);
            }
            if leaves < far {
                far = leaves;
                far_axis = Some(axis);
            }
            if near > far {
                return None;
            }
        }
        // A non-zero direction has a non-zero component, and that axis is not
        // parallel to its own slab, so it set both of these.
        Some(Crossing {
            near,
            far,
            near_axis: near_axis.expect("some axis decided the entry"),
            far_axis: far_axis.expect("some axis decided the exit"),
        })
    }
}

impl Crossing {
    /// The normal of the face the line enters through, turned to face the line.
    ///
    /// This is the box's own outward normal there — the entry face is the one
    /// pointing back at an approaching ray.
    pub fn entry_normal(&self, direction: Vec3) -> Vec3 {
        facing(self.near_axis, direction)
    }

    /// The normal of the face the line leaves through, turned to face the line.
    ///
    /// Here that is the *inward* normal: a ray leaving is looking at the inside
    /// of the far face. Which is what a bounce inside a body needs, and what
    /// makes taking [`Crossing::entry_normal`] for both ends wrong rather than
    /// merely imprecise — it names a face on the other side of the box.
    pub fn exit_normal(&self, direction: Vec3) -> Vec3 {
        facing(self.far_axis, direction)
    }
}

/// The unit vector along `axis` that points against `direction`.
fn facing(axis: usize, direction: Vec3) -> Vec3 {
    let sign = match direction.axis(axis) < 0.0 {
        true => 1.0,
        false => -1.0,
    };
    match axis {
        0 => Vec3::new(sign, 0.0, 0.0),
        1 => Vec3::new(0.0, sign, 0.0),
        _ => Vec3::new(0.0, 0.0, sign),
    }
}

#[cfg(test)]
mod tests {
    use super::{Aabb, Crossing};
    use crate::vector::Vec3;

    /// The unit cube at the origin, which every case below is worked out
    /// against by hand.
    fn unit() -> Aabb {
        Aabb::between(Vec3::ZERO, Vec3::splat(1.0))
    }

    #[test]
    fn a_ray_down_the_middle_enters_and_leaves_at_the_two_faces() {
        let crossing = unit()
            .crossing(Vec3::new(-1.0, 0.5, 0.5), Vec3::new(1.0, 0.0, 0.0))
            .expect("a ray straight at the box");
        assert_eq!(crossing.near, 1.0, "one unit of travel to reach x = 0");
        assert_eq!(crossing.far, 2.0, "and two to reach x = 1");
        assert_eq!(crossing.near_axis, 0, "the x slab decided the entry");
        assert_eq!(
            crossing.entry_normal(Vec3::new(1.0, 0.0, 0.0)),
            Vec3::new(-1.0, 0.0, 0.0),
            "the face it entered through points back at the ray"
        );
    }

    #[test]
    fn the_exit_normal_names_the_face_the_ray_leaves_through() {
        // Not the entry face's opposite in general: a ray through a box at an
        // angle enters on one axis and leaves on another, and using the entry
        // axis for both names a face on the wrong side of the box.
        // In across `x` at t=1, out across `y` at t=1.5: the `y` slab runs out
        // first because the ray climbs it slowly enough to enter behind the
        // box's own near face and leave through its far one.
        let direction = Vec3::new(1.0, 0.6, 0.0);
        let crossing = unit()
            .crossing(Vec3::new(-1.0, 0.1, 0.5), direction)
            .expect("in through x, out through y");
        assert_eq!(crossing.near_axis, 0, "entered across x");
        assert_eq!(crossing.far_axis, 1, "and left across y");
        assert_eq!(
            crossing.exit_normal(direction),
            Vec3::new(0.0, -1.0, 0.0),
            "the inside of the far face, turned to face the ray"
        );
    }

    #[test]
    fn the_parameter_is_in_units_of_the_direction_given() {
        // The property the shadow tests below depend on: pass `to - from` and
        // `near` reads as a fraction of the segment, so "before the light" is
        // `near < 1` and nothing has to know the distance.
        let crossing = unit()
            .crossing(Vec3::new(-2.0, 0.5, 0.5), Vec3::new(4.0, 0.0, 0.0))
            .expect("a ray straight at the box");
        assert_eq!(crossing.near, 0.5, "half of a four-unit step reaches x = 0");
    }

    #[test]
    fn a_ray_that_starts_inside_still_entered_the_box_behind_itself() {
        // A line, not a ray with an origin: it entered at a negative parameter.
        // Reporting "no entry" here would be a special case every caller has to
        // handle, and the thing it would protect against — using an entry face
        // that is behind you — is the caller's own clipping job anyway.
        let crossing = unit()
            .crossing(Vec3::splat(0.5), Vec3::new(1.0, 0.0, 0.0))
            .expect("a ray from inside still leaves");
        assert_eq!(crossing.near, -0.5, "half a unit back to the near face");
        assert_eq!(crossing.far, 0.5, "and half a unit on to the far one");
        assert_eq!(crossing.near_axis, 0);
        assert_eq!(crossing.far_axis, 0);
    }

    #[test]
    fn a_box_behind_the_origin_crosses_at_a_negative_parameter() {
        // A line and not a segment: the caller clips. A `None` here instead
        // would quietly make every intersection routine above depend on this
        // one's idea of which way is forward.
        let crossing = unit()
            .crossing(Vec3::new(3.0, 0.5, 0.5), Vec3::new(1.0, 0.0, 0.0))
            .expect("the line still meets the box");
        assert_eq!((crossing.near, crossing.far), (-3.0, -2.0), "both behind");
    }

    #[test]
    fn running_along_a_face_grazes_it_and_does_not_cross_it() {
        // Exactly along the `z = 1` lid. There is a segment of the line whose
        // x and y are inside the box, but no point of it is ever *in* the box,
        // so an interval here would enclose no interior at all — and a shadow
        // test believing it would draw a hard line along every silhouette edge
        // in the scene.
        assert_eq!(
            unit().crossing(Vec3::new(-1.0, 0.5, 1.0), Vec3::new(1.0, 0.0, 0.0)),
            None,
            "along the lid is not through the box"
        );
        // A hair below the lid is an ordinary crossing, and this is what says
        // the rule above is about the boundary and not about a tolerance.
        assert_eq!(
            unit()
                .crossing(Vec3::new(-1.0, 0.5, 1.0 - 1e-12), Vec3::new(1.0, 0.0, 0.0))
                .map(|c| (c.near, c.far)),
            Some((1.0, 2.0)),
            "inside by any margin at all is inside"
        );
    }

    #[test]
    fn a_ray_inside_a_plane_never_crosses_it() {
        // The same rule with both bounds coincident. Answering "blocked" here
        // would have every light at exactly a floor's height shadowed by that
        // floor along its whole length.
        let floor = Aabb::between(Vec3::ZERO, Vec3::new(1.0, 1.0, 0.0));
        assert_eq!(
            floor.crossing(Vec3::new(-1.0, 0.5, 0.0), Vec3::new(1.0, 0.0, 0.0)),
            None,
            "a plane is crossed, never travelled through"
        );
        assert_eq!(
            floor
                .crossing(Vec3::new(0.5, 0.5, -1.0), Vec3::new(0.0, 0.0, 1.0))
                .map(|c| (c.near, c.far)),
            Some((1.0, 1.0)),
            "but crossing it at an angle is an ordinary zero-length crossing"
        );
    }

    #[test]
    fn a_miss_is_a_miss_on_every_axis() {
        let cube = unit();
        for (from, direction, why) in [
            (
                Vec3::new(-1.0, 2.0, 0.5),
                Vec3::new(1.0, 0.0, 0.0),
                "passes north of it",
            ),
            (
                Vec3::new(-1.0, 0.5, 2.0),
                Vec3::new(1.0, 0.0, 0.0),
                "passes over it",
            ),
            (
                Vec3::new(0.0, 0.0, 5.0),
                Vec3::new(1.0, -1.0, 0.0),
                "runs level, well above it",
            ),
        ] {
            assert_eq!(cube.crossing(from, direction), None, "{why}");
        }
    }

    #[test]
    fn a_diagonal_through_the_cubes_corners_enters_at_zero_and_leaves_at_one() {
        let crossing = unit().crossing(Vec3::ZERO, Vec3::splat(1.0));
        assert_eq!(
            crossing.map(|c| (c.near, c.far)),
            Some((0.0, 1.0)),
            "the body diagonal, corner to corner"
        );
    }

    /// A property no single worked case states: the interval a ray spends
    /// inside a box does not depend on which end of it you start from.
    ///
    /// Reversing the ray has to reverse the interval exactly — `near` and `far`
    /// swap and both are measured back from the far end. It catches an
    /// asymmetric epsilon, an inequality that is strict on one side only, and a
    /// swap that fires on one sign of the direction, none of which a hand-picked
    /// case is likely to sit on.
    #[test]
    fn reversing_a_ray_reverses_the_interval_it_spends_inside() {
        use proptest::prelude::*;
        proptest!(|(
            ox in -5.0f64..5.0, oy in -5.0f64..5.0, oz in -5.0f64..5.0,
            dx in -3.0f64..3.0, dy in -3.0f64..3.0, dz in -3.0f64..3.0,
        )| {
            let direction = Vec3::new(dx, dy, dz);
            prop_assume!(direction.length() > 0.1);
            let from = Vec3::new(ox, oy, oz);
            // The same line, walked from the other end: `from + direction` is
            // where the forward ray's parameter is 1, so a backward parameter
            // `t` is a forward parameter `1 - t`.
            let there = unit().crossing(from, direction);
            let back = unit().crossing(from + direction, -direction);
            match (there, back) {
                (None, None) => {}
                (Some(there), Some(back)) => {
                    prop_assert!((there.near - (1.0 - back.far)).abs() < 1e-9, "{there:?} {back:?}");
                    prop_assert!((there.far - (1.0 - back.near)).abs() < 1e-9, "{there:?} {back:?}");
                }
                (there, back) => prop_assert!(false, "one direction hit and the other missed: {there:?} {back:?}"),
            }
        });
    }

    /// And the one property that ties `crossing` to what it is *for*: a point
    /// strictly inside the interval is strictly inside the box.
    ///
    /// This is the statement a shadow test actually relies on, and it is not
    /// implied by any of the worked cases above — those check where the
    /// boundaries are, this checks that the interval between them means what
    /// the name says.
    #[test]
    fn every_point_inside_the_reported_interval_is_inside_the_box() {
        use proptest::prelude::*;
        proptest!(|(
            ox in -5.0f64..5.0, oy in -5.0f64..5.0, oz in -5.0f64..5.0,
            dx in -3.0f64..3.0, dy in -3.0f64..3.0, dz in -3.0f64..3.0,
            through in 0.05f64..0.95,
        )| {
            let direction = Vec3::new(dx, dy, dz);
            prop_assume!(direction.length() > 0.1);
            let from = Vec3::new(ox, oy, oz);
            let Some(Crossing { near, far, .. }) = unit().crossing(from, direction) else {
                return Ok(());
            };
            prop_assume!(far - near > 1e-6);
            let at = from + direction * (near + (far - near) * through);
            let cube = unit();
            for axis in 0..3 {
                prop_assert!(
                    at.axis(axis) >= cube.min.axis(axis) - 1e-9
                        && at.axis(axis) <= cube.max.axis(axis) + 1e-9,
                    "axis {axis} of {at:?} left the box between near {near} and far {far}"
                );
            }
        });
    }
}
