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
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}
