//! Three numbers, and the arithmetic every other module here is written in.
//!
//! `f64` throughout. The renderer is `f32` on the GPU and mixed on the CPU, and
//! this crate exists partly to say whether a disagreement is a rounding
//! disagreement — which it cannot do from inside the same precision.

use std::ops::{Add, Div, Mul, Neg, Sub};

/// A point in the world, or a direction through it.
///
/// One type for both, deliberately: the distinction is in what a function does
/// with it, and a `Point`/`Direction` split here would spend its whole life
/// being converted at the boundary between the two in every intersection
/// routine. What the units *are* is [`crate::scene::Scene`]'s subject.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Vec3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl Vec3 {
    pub const ZERO: Self = Self::new(0.0, 0.0, 0.0);

    pub const fn new(x: f64, y: f64, z: f64) -> Self {
        Self { x, y, z }
    }

    /// The same number on all three axes — a tolerance, a uniform scale.
    pub const fn splat(value: f64) -> Self {
        Self::new(value, value, value)
    }

    pub fn dot(self, other: Self) -> f64 {
        self.x * other.x + self.y * other.y + self.z * other.z
    }

    pub fn cross(self, other: Self) -> Self {
        Self::new(
            self.y * other.z - self.z * other.y,
            self.z * other.x - self.x * other.z,
            self.x * other.y - self.y * other.x,
        )
    }

    pub fn length(self) -> f64 {
        self.dot(self).sqrt()
    }

    /// The direction alone.
    ///
    /// # Panics
    ///
    /// On a zero-length vector. A direction of no length is not a direction
    /// with a sensible default — it is a caller that has lost track of two
    /// coincident points, and returning some unit vector would hide that inside
    /// a shadow ray that then reports whatever the arbitrary choice implied.
    pub fn normalized(self) -> Self {
        let length = self.length();
        assert!(
            length > 0.0,
            "normalizing a zero-length vector: two points that should differ are the same one"
        );
        self / length
    }

    /// Componentwise minimum — a box's own `min` corner, built from two corners
    /// given in any order.
    pub fn min(self, other: Self) -> Self {
        Self::new(self.x.min(other.x), self.y.min(other.y), self.z.min(other.z))
    }

    pub fn max(self, other: Self) -> Self {
        Self::new(self.x.max(other.x), self.y.max(other.y), self.z.max(other.z))
    }

    /// The axis, `0..3`, this vector is largest on — how a normal picks the
    /// slab that produced it.
    pub fn axis(self, axis: usize) -> f64 {
        match axis {
            0 => self.x,
            1 => self.y,
            2 => self.z,
            other => panic!("axis {other} of three"),
        }
    }

    /// Some unit vector perpendicular to this one, and a second perpendicular to
    /// both: an orthonormal frame around a surface normal, for sampling a
    /// hemisphere around it.
    ///
    /// The branch on the largest component is what keeps the cross product from
    /// being taken against a nearly parallel axis, where its length — and so the
    /// frame's accuracy — collapses. `self` must already be normalized.
    pub fn orthonormal_basis(self) -> (Self, Self) {
        let away = match self.x.abs() > 0.9 {
            true => Self::new(0.0, 1.0, 0.0),
            false => Self::new(1.0, 0.0, 0.0),
        };
        let u = self.cross(away).normalized();
        (u, self.cross(u))
    }
}

impl Add for Vec3 {
    type Output = Self;
    fn add(self, other: Self) -> Self {
        Self::new(self.x + other.x, self.y + other.y, self.z + other.z)
    }
}

impl Sub for Vec3 {
    type Output = Self;
    fn sub(self, other: Self) -> Self {
        Self::new(self.x - other.x, self.y - other.y, self.z - other.z)
    }
}

impl Neg for Vec3 {
    type Output = Self;
    fn neg(self) -> Self {
        Self::new(-self.x, -self.y, -self.z)
    }
}

impl Mul<f64> for Vec3 {
    type Output = Self;
    fn mul(self, scale: f64) -> Self {
        Self::new(self.x * scale, self.y * scale, self.z * scale)
    }
}

impl Div<f64> for Vec3 {
    type Output = Self;
    fn div(self, divisor: f64) -> Self {
        Self::new(self.x / divisor, self.y / divisor, self.z / divisor)
    }
}

#[cfg(test)]
mod tests {
    use super::Vec3;

    #[test]
    fn a_cross_product_is_perpendicular_to_both_of_its_arguments() {
        let (a, b) = (Vec3::new(1.0, 2.0, 3.0), Vec3::new(-4.0, 5.0, 0.5));
        let c = a.cross(b);
        assert!(c.dot(a).abs() < 1e-12, "perpendicular to the first");
        assert!(c.dot(b).abs() < 1e-12, "perpendicular to the second");
    }

    #[test]
    fn an_orthonormal_basis_is_orthonormal_even_around_an_axis_aligned_normal() {
        // The axis-aligned normals are every normal this crate's boxes have, so
        // the degenerate cross product the branch in `orthonormal_basis` exists
        // to avoid is the ordinary case here rather than a corner one.
        for normal in [
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            Vec3::new(0.0, 0.0, 1.0),
            Vec3::new(-1.0, 0.0, 0.0),
            Vec3::new(0.6, -0.8, 0.0),
        ] {
            let (u, v) = normal.orthonormal_basis();
            assert!(
                (u.length() - 1.0).abs() < 1e-12,
                "u is a unit vector at {normal:?}"
            );
            assert!(
                (v.length() - 1.0).abs() < 1e-12,
                "v is a unit vector at {normal:?}"
            );
            assert!(u.dot(v).abs() < 1e-12, "u ⟂ v at {normal:?}");
            assert!(u.dot(normal).abs() < 1e-12, "u ⟂ n at {normal:?}");
            assert!(v.dot(normal).abs() < 1e-12, "v ⟂ n at {normal:?}");
        }
    }
}
