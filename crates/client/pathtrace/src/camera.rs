//! The camera, recovered from the renderer's own projection by measuring it.
//!
//! A path tracer needs the inverse of what a rasterizer needs: not "where does
//! this world point land on screen" but "which world points land on this
//! pixel". Deriving that by re-deriving the projection's formula is the one
//! thing this crate must not do — a second copy of a formula is the shape that
//! drifts, and a reference camera that has drifted from the real one produces a
//! picture of a scene nobody rendered.
//!
//! So the projection arrives as a **black box**: a function from a world point
//! to a pixel, which the caller builds out of the renderer's own code. This
//! module measures it at a handful of points, recovers the affine map, and then
//! checks that assumption on probes it did not measure from. Nothing here knows
//! that a tile is 44 pixels across or that a `z` unit lifts a sprite four.

use crate::vector::Vec3;

/// A line through the world: every point that projects to one pixel.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Ray {
    /// A point on it — the one closest to the world origin, which is an
    /// arbitrary choice with one useful property: it does not depend on how far
    /// away a caller imagined the camera to be.
    pub at: Vec3,
    /// Which way the viewer is looking: a unit vector, pointing *into* the
    /// scene. Increasing `t` along it goes away from the viewer, so a smaller
    /// `t` is nearer and the front-most surface is the smallest one.
    pub direction: Vec3,
}

impl Ray {
    pub fn point(&self, t: f64) -> Vec3 {
        self.at + self.direction * t
    }
}

/// A parallel projection, recovered by measurement.
///
/// Parallel and not perspective: this is what an isometric renderer has, and it
/// is what makes the recovery possible at all — an affine map from three
/// numbers to two, whose null space is a single direction shared by every
/// pixel's ray.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Parallel {
    /// The pixel a step of one along each world axis moves by.
    columns: [(f64, f64); 3],
    /// The pixel the world origin projects to.
    origin: (f64, f64),
    /// The null space of `columns`, oriented to point away from the viewer.
    direction: Vec3,
}

impl Parallel {
    /// Recover the projection `map` describes.
    ///
    /// - `about` — where to measure. Put it in the scene: `map` is very likely
    ///   `f32` somewhere inside, and a difference of two large coordinates
    ///   loses digits, so measuring the projection of a far-away world near the
    ///   scene it will be used on is worth real precision.
    /// - `span` — how far apart to take the two probes of each axis. A central
    ///   difference over a wide span divides whatever noise `map` has by twice
    ///   the span; too wide and the probes leave the region where the caller
    ///   cares whether the recovery is exact. Tens of world units is right for
    ///   a projection with `f32` in it.
    /// - `tolerance` — how many pixels of disagreement the linearity check
    ///   allows. This is a claim about `map`'s own noise, not about the
    ///   scene: set it well under a pixel, and well over what the probes' own
    ///   arithmetic can produce.
    ///
    /// # Panics
    ///
    /// - When `map` is not affine to within `tolerance` on probe points that
    ///   are not the ones it was measured from. That is the assumption this
    ///   whole module rests on, and a projection with any perspective in it —
    ///   or with a clamp, or a rounding, anywhere in its path — has to fail
    ///   loudly here rather than silently produce rays that are not the
    ///   renderer's.
    /// - When the recovered map has rank below two (it collapses the world to a
    ///   line or a point), so there is no single ray direction to speak of.
    /// - When the ray direction it recovers is horizontal. The null space comes
    ///   out of the arithmetic without a sign — a line has two ends — and the
    ///   one thing this module assumes about which end the viewer is on is that
    ///   the viewer is *above*. That is checkable in the picture rather than
    ///   argued: a camera under the world would draw the ground over the things
    ///   standing on it.
    pub fn measure(map: impl Fn(Vec3) -> (f64, f64), about: Vec3, span: f64, tolerance: f64) -> Self {
        assert!(span > 0.0, "a measurement span of {span} probes one point twice");
        let axis_step = |axis: usize| match axis {
            0 => Vec3::new(span, 0.0, 0.0),
            1 => Vec3::new(0.0, span, 0.0),
            _ => Vec3::new(0.0, 0.0, span),
        };
        let mut columns = [(0.0, 0.0); 3];
        for (axis, column) in columns.iter_mut().enumerate() {
            let step = axis_step(axis);
            let ahead = map(about + step);
            let behind = map(about - step);
            *column = (
                (ahead.0 - behind.0) / (2.0 * span),
                (ahead.1 - behind.1) / (2.0 * span),
            );
        }
        let centre = map(about);
        let origin = (
            centre.0
                - columns
                    .iter()
                    .enumerate()
                    .map(|(a, c)| c.0 * about.axis(a))
                    .sum::<f64>(),
            centre.1
                - columns
                    .iter()
                    .enumerate()
                    .map(|(a, c)| c.1 * about.axis(a))
                    .sum::<f64>(),
        );
        let recovered = Self {
            columns,
            origin,
            // Filled in below; the linearity check runs through `pixel_of`,
            // which does not read it.
            direction: Vec3::new(0.0, 0.0, -1.0),
        };

        // The check the doc promises. Deliberately awkward offsets: not on an
        // axis, not a multiple of `span`, both signs, and one far enough out
        // that a projection with any curvature in it has somewhere to curve.
        // A probe on the measurement points themselves would pass by
        // construction and say nothing.
        for offset in [
            Vec3::new(0.37, -1.25, 2.5),
            Vec3::new(-3.1, 0.8, -1.75),
            Vec3::new(11.0, 13.0, -7.0),
            Vec3::new(-0.05, -0.05, 0.05),
        ] {
            let at = about + offset * span;
            let (want, got) = (map(at), recovered.pixel_of(at));
            let off = ((want.0 - got.0).powi(2) + (want.1 - got.1).powi(2)).sqrt();
            assert!(
                off <= tolerance,
                "the projection is not affine: {at:?} lands at {want:?}, but the map recovered from \
                 three axis probes says {got:?} — {off} pixels out, tolerance {tolerance}"
            );
        }

        // The ray direction is the null space of the 2x3 matrix: the one
        // direction whose pixel offset is zero on both rows. For a 2x3 that is
        // exactly the cross product of the two rows, with no linear solve.
        let rows = (
            Vec3::new(columns[0].0, columns[1].0, columns[2].0),
            Vec3::new(columns[0].1, columns[1].1, columns[2].1),
        );
        let kernel = rows.0.cross(rows.1);
        assert!(
            kernel.length() > 1e-9 * rows.0.length() * rows.1.length(),
            "the projection has rank below two — it collapses the world onto a line, so a pixel \
             names a plane of world points and not a ray"
        );
        assert!(
            kernel.z.abs() > 1e-9 * kernel.length(),
            "the recovered view direction is horizontal, so there is no upper end of it to put the \
             viewer at; this module cannot tell which way a side-on camera faces"
        );
        // Oriented so the viewer is above: looking into the scene is looking
        // down. The kernel comes out of a cross product with an arbitrary sign,
        // and this is the only place a fact about the renderer is written down
        // rather than measured.
        let direction = match kernel.z > 0.0 {
            true => -kernel.normalized(),
            false => kernel.normalized(),
        };
        Self {
            direction,
            ..recovered
        }
    }

    /// The projection as recovered — the caller's own `map`, if the measurement
    /// was exact.
    ///
    /// Public because a caller that has both should compare them over its own
    /// scene: this module's linearity check probes four points, and the caller
    /// knows which points its picture is actually about.
    pub fn pixel_of(&self, at: Vec3) -> (f64, f64) {
        let mut pixel = self.origin;
        for (axis, column) in self.columns.iter().enumerate() {
            pixel.0 += column.0 * at.axis(axis);
            pixel.1 += column.1 * at.axis(axis);
        }
        pixel
    }

    /// Which way the viewer looks. A unit vector into the scene.
    pub fn direction(&self) -> Vec3 {
        self.direction
    }

    /// The world line that projects to `pixel`.
    ///
    /// Two equations in three unknowns, so the solution is a line rather than a
    /// point, which is the whole reason this returns a [`Ray`]. The point it
    /// picks on that line is the least-norm one — the normal equations of the
    /// 2x3 system, whose 2x2 matrix is invertible exactly when the rank check in
    /// [`Parallel::measure`] passed.
    pub fn ray(&self, pixel: (f64, f64)) -> Ray {
        let rows = (
            Vec3::new(self.columns[0].0, self.columns[1].0, self.columns[2].0),
            Vec3::new(self.columns[0].1, self.columns[1].1, self.columns[2].1),
        );
        let target = (pixel.0 - self.origin.0, pixel.1 - self.origin.1);
        let (a, b, c) = (rows.0.dot(rows.0), rows.0.dot(rows.1), rows.1.dot(rows.1));
        let determinant = a * c - b * b;
        // `measure` proved the two rows independent, and a Gram determinant of
        // two independent vectors is strictly positive.
        let (u, v) = (
            (c * target.0 - b * target.1) / determinant,
            (a * target.1 - b * target.0) / determinant,
        );
        Ray {
            at: rows.0 * u + rows.1 * v,
            direction: self.direction,
        }
    }

    /// Where a world point sits along its own pixel's ray.
    ///
    /// The depth a painter's-order renderer is sorting by, in this crate's own
    /// terms: smaller is nearer. Useful to a caller comparing which of two
    /// surfaces a pixel belongs to.
    pub fn depth_of(&self, at: Vec3) -> f64 {
        at.dot(self.direction)
    }
}

#[cfg(test)]
mod tests {
    use super::Parallel;
    use crate::vector::Vec3;

    /// The renderer's own projection, to the digit — `camera::project_exact`
    /// composed with a 4:1 zoom about the middle of a 512-pixel frame.
    ///
    /// Written out here rather than imported, because importing it is exactly
    /// what this crate does not do. It is a fixture for testing the *recovery*,
    /// and if the renderer's projection ever changes shape the recovery does not
    /// care — `measure` never saw this formula, only its values.
    fn openshard_like(at: Vec3) -> (f64, f64) {
        let (half_width, half_height, z_step) = (22.0, 22.0, 4.0);
        let world = (
            (at.x - at.y) * half_width,
            (at.x + at.y - 1.0) * half_height - at.z * z_step,
        );
        let (eye, scale) = ((4400.0, 4378.0), 4.0);
        (
            (world.0 - eye.0) * scale + 256.0,
            (world.1 - eye.1) * scale + 256.0,
        )
    }

    fn measured() -> Parallel {
        Parallel::measure(openshard_like, Vec3::new(100.5, 100.5, 3.0), 32.0, 1e-6)
    }

    #[test]
    fn the_recovered_projection_reproduces_the_one_it_measured() {
        let camera = measured();
        for at in [
            Vec3::new(100.5, 100.5, 3.0),
            Vec3::new(97.25, 103.75, 0.0),
            Vec3::new(-14.0, 250.5, 27.5),
        ] {
            let (want, got) = (openshard_like(at), camera.pixel_of(at));
            assert!(
                (want.0 - got.0).abs() < 1e-6 && (want.1 - got.1).abs() < 1e-6,
                "{at:?}: measured {got:?}, real {want:?}"
            );
        }
    }

    #[test]
    fn every_point_of_a_pixels_ray_projects_back_to_that_pixel() {
        // The property that makes the ray a ray. It is what the whole tracer
        // stands on: if it fails, every pixel is asking about a line through
        // some other pixel's world.
        let camera = measured();
        for pixel in [(0.0, 0.0), (256.5, 300.25), (511.0, 12.0)] {
            let ray = camera.ray(pixel);
            for t in [-1000.0, -3.5, 0.0, 17.0, 4000.0] {
                let back = camera.pixel_of(ray.point(t));
                assert!(
                    (back.0 - pixel.0).abs() < 1e-6 && (back.1 - pixel.1).abs() < 1e-6,
                    "pixel {pixel:?} at t={t} came back as {back:?}"
                );
            }
        }
    }

    #[test]
    fn the_recovered_view_direction_is_the_projections_own_kernel() {
        // Eleven `z` units to a tile is the renderer's own `Z_PER_TILE`, and it
        // falls out of the projection here rather than being told to this crate:
        // a step of one `z` lifts a sprite four pixels, a step of one tile moves
        // it twenty-two, so the direction that changes no pixel at all is
        // (1, 1, 11) — in these units, since `openshard_like` above takes `z` in
        // the map's own units and not in tiles.
        let direction = measured().direction();
        let want = Vec3::new(-1.0, -1.0, -11.0).normalized();
        assert!(
            (direction - want).length() < 1e-9,
            "recovered {direction:?}, wanted {want:?}"
        );
        assert!(direction.z < 0.0, "the viewer is above the world");
    }

    #[test]
    fn a_world_point_and_its_own_pixels_ray_agree_about_depth() {
        let camera = measured();
        let (low, high) = (Vec3::new(100.5, 100.5, 0.0), Vec3::new(100.5, 100.5, 6.0));
        assert!(
            camera.depth_of(high) < camera.depth_of(low),
            "the higher of two points on one column is the nearer to a camera above"
        );
    }

    #[test]
    #[should_panic(expected = "not affine")]
    fn a_projection_with_perspective_in_it_is_refused() {
        // The check earning its keep: divide by depth and the map stops being
        // affine, which is a camera this crate cannot invert to a single
        // direction. It must say so rather than recover a plausible-looking
        // parallel camera that is nobody's.
        Parallel::measure(
            |at| {
                let w = 40.0 - at.z;
                ((at.x - at.y) * 22.0 / w, (at.x + at.y) * 22.0 / w)
            },
            Vec3::new(100.5, 100.5, 3.0),
            32.0,
            1e-6,
        );
    }

    #[test]
    #[should_panic(expected = "rank below two")]
    fn a_projection_that_collapses_the_world_to_a_line_is_refused() {
        Parallel::measure(
            |at| {
                let along = at.x + at.y + at.z;
                (along, 2.0 * along)
            },
            Vec3::ZERO,
            32.0,
            1e-6,
        );
    }

    #[test]
    #[should_panic(expected = "horizontal")]
    fn a_side_on_camera_is_refused_because_up_cannot_be_told_from_down() {
        // A plain elevation: `x` across, `z` up the screen, `y` into it. Its
        // kernel is the `y` axis, which is horizontal, so "the viewer is above"
        // says nothing about which side they are on.
        Parallel::measure(|at| (at.x, -at.z), Vec3::ZERO, 32.0, 1e-6);
    }

    #[test]
    fn measuring_survives_a_projection_that_rounds_to_f32() {
        // The real one does: the renderer's own `to_view_exact` narrows to `f32`
        // at world-pixel magnitudes in the thousands, so the black box this
        // module measures is noisy at about a ten-thousandth of a pixel. The
        // wide central difference is what keeps that out of the columns, and
        // this is the test that says so — with a tolerance loose enough for the
        // noise and far tighter than a pixel.
        let noisy = |at: Vec3| {
            let exact = openshard_like(at);
            (f64::from(exact.0 as f32), f64::from(exact.1 as f32))
        };
        let camera = Parallel::measure(noisy, Vec3::new(100.5, 100.5, 3.0), 32.0, 1e-2);
        let want = Vec3::new(-1.0, -1.0, -11.0).normalized();
        assert!(
            (camera.direction() - want).length() < 1e-5,
            "an `f32` projection still recovers its own kernel: {:?}",
            camera.direction()
        );
    }
}
