//! A reference path tracer: the same scene, answered from a different set of
//! ideas.
//!
//! # Why a second renderer at all
//!
//! `openshard-client-render` decides a shadow with a DDA over tiles: a ray
//! carries the tile it starts in, steps to whichever cell boundary is nearer,
//! and asks each candidate cell's occluders whether they stop it. That walk has
//! a rich vocabulary — cells, boundaries, stances, sub-tile fractions, a
//! fragment's own surface and the exemption that follows from it — and every
//! term in it is a place a defect can live. The oracles that crate already runs
//! (`examples/oracle`'s `segment_clear_of_box`) are independent *arithmetic*,
//! which is worth a great deal, but they answer one question at a time about a
//! point somebody chose.
//!
//! This crate has none of that vocabulary. There is no tile here, no cell, no
//! boundary, no stance, no fragment. There is a ray, a box, and where they
//! meet. So a whole class of defect — everything that can only be said in the
//! walk's own words — cannot be reproduced here by construction rather than by
//! coverage. That is the claim this crate makes, and it is why it has no
//! dependencies: a shared helper would be a word from the other vocabulary
//! sneaking in.
//!
//! It is also a **third party**. Where `light::sample` (CPU) and `blit.wesl`
//! (GPU) disagree, both are implementations of one formula and neither can say
//! which of them is right. This one is not in that argument.
//!
//! # The two ways to run it, and why both
//!
//! - **Degenerate** — a point emitter, one shadow sample, no bounces
//!   ([`trace::Settings::degenerate`]). Monte Carlo with nothing random left in
//!   it: the estimator collapses to a single deterministic visibility test per
//!   pixel, which is exactly the question the renderer's hard shadows answer.
//!   In this mode the two pictures **must** agree, and a disagreement is a
//!   defect in one of them. This is the gate.
//! - **Full** — a spherical emitter, many samples, diffuse bounces. The two
//!   pictures will *not* agree and must not be compared pixel for pixel. This
//!   mode answers a different question: what the same geometry looks like when
//!   penumbra, the cosine term and one bounce of indirect light exist at all.
//!   It is not a check, it is a look at what the shipped model does not
//!   contain.
//!
//! Running both from one body of code is the point. A reference that can only
//! do the beautiful version proves nothing about the shipped one, and a
//! reference that can only do the degenerate version has no opinion about
//! anything the shipped model left out.
//!
//! # What it assumes about the renderer, in full
//!
//! Two things, and they are both stated where they are used rather than
//! compiled in here:
//!
//! 1. **The world-to-pixel map is affine.** [`camera::Parallel::measure`] takes
//!    that map as a black box, recovers it by measuring, and asserts the
//!    linearity it assumed on probe points it did not measure from. Nothing
//!    here copies the projection's formula, so nothing here can drift from it.
//! 2. **The world has a metric.** Boxes and lights arrive in whatever units the
//!    caller uses, and this crate treats those units as isotropic and
//!    Euclidean. Visibility does not care — it survives any affine change of
//!    coordinates — but a cosine, a solid angle and a falloff all do, so a
//!    caller whose axes are not in one unit has to scale them on the way in.
//!    See [`scene::Scene`].
//!
//! # Determinism
//!
//! Every pixel draws from its own stream, seeded from its own coordinates and
//! the settings' seed ([`rng::Stream`]). The same inputs give the same image,
//! on any machine, at any sample count, whatever order the pixels are visited
//! in — a reference whose output moves between runs cannot be the thing another
//! picture is measured against.

pub mod aabb;
pub mod camera;
pub mod light;
pub mod rng;
pub mod scene;
pub mod trace;
pub mod vector;
