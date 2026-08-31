//! The two shapes every screen-space number in this crate turns out to be.
//!
//! Not a math library: no dot product, no matrix, nothing this crate does not
//! call. [`Camera`](crate::camera::Camera) already gives the world's own
//! coordinates a type per space — [`WorldPixel`](crate::camera::WorldPixel),
//! [`WorldPoint`](crate::camera::WorldPoint), [`ViewPixel`](crate::camera::ViewPixel)
//! — and stops at whole pixels, because a body mid-step is the only fraction
//! anywhere in world space. Past the camera, in viewport and atlas pixels, the
//! fraction is ordinary and everything is `f32`; what was missing there was not
//! a space of its own but a name for "two of them" and "four of them together"
//! — which is what a bare `(f32, f32)` scattered through a signature always
//! turns out to be.

/// A point or an offset in one of this crate's `f32` pixel spaces.
///
/// Which space is up to the doc comment on whatever holds it — a fractional
/// [`ViewPixel`](crate::camera::ViewPixel), a viewport pixel after
/// [`Camera::to_viewport`](crate::camera::Camera::to_viewport) — the same way
/// two bare `i32`s are [`WorldPixel`](crate::camera::WorldPixel) in one
/// function and [`ViewPixel`](crate::camera::ViewPixel) in another. No `From`
/// or `Into`: a value that crossed from one space to another without a
/// [`Camera`](crate::camera::Camera) doing the arithmetic would be exactly the
/// silent mistake the world-space types already refuse.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

impl Vec2 {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

/// An axis-aligned rectangle: a top-left corner and an extent, in the same
/// pixel space as the corner.
///
/// `width` and `height` are always non-negative here — a sprite's own size,
/// never the sign [`SpriteQuad::mirrored`](crate::sprite::SpriteQuad::mirrored)
/// puts on a [`Region`](crate::atlas::Region) to sample it backwards. That is
/// a different rectangle, in atlas UV space rather than pixels, and it stays
/// its own type for the same reason [`WorldPixel`](crate::camera::WorldPixel)
/// and [`ViewPixel`](crate::camera::ViewPixel) do not share one either.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Rect {
    pub x:      f32,
    pub y:      f32,
    pub width:  f32,
    pub height: f32,
}

impl Rect {
    /// Whether this rectangle and `other` share any pixel, in the space both
    /// are stated in — a caller's to keep true, the same way every other
    /// function in this module leaves the space to its doc comment rather than
    /// the type.
    ///
    /// Touching edges do not count: `<`/`>`, not `<=`/`>=`, which is the usual
    /// convention for a half-open pixel rectangle and keeps two rectangles that
    /// merely share a border from reading as overlapping.
    #[must_use]
    pub fn intersects(&self, other: &Self) -> bool {
        self.x < other.x + other.width
            && other.x < self.x + self.width
            && self.y < other.y + other.height
            && other.y < self.y + self.height
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f32, y: f32, width: f32, height: f32) -> Rect {
        Rect { x, y, width, height }
    }

    #[test]
    fn overlapping_rectangles_intersect() {
        let a = rect(0.0, 0.0, 10.0, 10.0);
        let b = rect(5.0, 5.0, 10.0, 10.0);
        assert!(a.intersects(&b));
        assert!(b.intersects(&a), "the test is symmetric in its own arguments");
    }

    #[test]
    fn disjoint_rectangles_do_not_intersect() {
        let a = rect(0.0, 0.0, 10.0, 10.0);
        let b = rect(20.0, 20.0, 10.0, 10.0);
        assert!(!a.intersects(&b));
    }

    /// A shared edge is not a shared pixel — the half-open convention this
    /// crate's other pixel-space tests already use (`statics::on_screen`).
    #[test]
    fn touching_edges_do_not_intersect() {
        let a = rect(0.0, 0.0, 10.0, 10.0);
        let b = rect(10.0, 0.0, 10.0, 10.0);
        assert!(!a.intersects(&b));
    }

    #[test]
    fn one_rectangle_inside_another_intersects() {
        let outer = rect(0.0, 0.0, 100.0, 100.0);
        let inner = rect(40.0, 40.0, 5.0, 5.0);
        assert!(outer.intersects(&inner));
        assert!(inner.intersects(&outer));
    }
}
