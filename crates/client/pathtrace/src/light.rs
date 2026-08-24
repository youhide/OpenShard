//! What is emitting, and how its brightness falls off with distance.
//!
//! Two axes, kept apart on purpose, because they answer different questions
//! about the renderer being checked:
//!
//! - **The emitter's shape** ([`Emitter`]) decides whether a shadow has an edge
//!   or a penumbra. A point emitter is what the renderer models, and it is what
//!   makes a Monte Carlo estimator collapse to one deterministic test.
//! - **The falloff** ([`Falloff`]) decides brightness alone and touches no
//!   geometry at all. It is a separate knob so that a picture can differ from
//!   the renderer's in exactly one of the two: physical light with the
//!   renderer's own hard shadows says something about the light model, soft
//!   shadows with the renderer's own falloff says something about the geometry,
//!   and a picture that changed both at once says neither.

use crate::rng::Stream;
use crate::vector::Vec3;

/// An index into the lights supplied to a render, never a pixel coordinate or
/// an index into another per-frame buffer.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct LightIdx(usize);

impl LightIdx {
    /// Name an entry in the caller's light list.
    pub const fn new(index: usize) -> Self {
        Self(index)
    }

    /// Its position in the light list passed to the render.
    ///
    /// This is deliberately not a general-purpose raw number: it may index
    /// only the lights whose visibility the rendered image records.
    pub const fn in_light_list(self) -> usize {
        self.0
    }
}

/// The shape of the thing emitting.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Emitter {
    /// All of it at one place. Casts a shadow with no penumbra: every point
    /// either sees it or does not, so one shadow ray answers exactly, and the
    /// answer has no variance to average away.
    Point,
    /// A sphere of this radius, **sampled over the silhouette it presents** —
    /// the disc of that radius facing whatever is being lit.
    ///
    /// So the radius is a purely geometric knob: it decides how wide a penumbra
    /// is and nothing else, because the brightness is taken from the emitter's
    /// centre either way. Widening it softens a shadow without changing the
    /// exposure, which is what makes a series of renders at growing radii a
    /// picture of the penumbra alone.
    Sphere { radius: f64 },
}

/// How brightness falls off with distance.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Falloff {
    /// `(1 - d / reach)²`, and nothing at all past `reach`.
    ///
    /// The renderer's own — not physics, a windowed curve chosen so a torch has
    /// an edge. Carried here so a reference picture can isolate the geometry it
    /// is really about from a brightness model it is not.
    Windowed { reach: f64 },
    /// `1 / d²`. What a real emitter does, and what the renderer's own curve is
    /// worth comparing against.
    InverseSquare,
}

impl Falloff {
    /// The multiplier at distance `d`, or [`None`] where this falloff says no
    /// light arrives at all.
    ///
    /// [`None`] and not zero: "outside the torch's reach" and "in shadow" are
    /// opposite facts about a dark pixel, and a caller comparing against a
    /// renderer that draws them in different colours has to be able to tell
    /// them apart. The renderer's own debug view spends a colour on exactly
    /// this distinction.
    pub fn at(self, d: f64) -> Option<f64> {
        match self {
            Self::Windowed { reach } => {
                let ratio = d / reach.max(f64::MIN_POSITIVE);
                match ratio >= 1.0 {
                    true => None,
                    false => Some((1.0 - ratio).powi(2)),
                }
            }
            // No singularity at zero in practice — a surface point is never
            // exactly on an emitter — but a scene can be authored with one, and
            // an infinity that propagates into an averaged pixel destroys the
            // whole image rather than one sample.
            Self::InverseSquare => match d > 0.0 {
                true => Some(1.0 / (d * d)),
                false => None,
            },
        }
    }
}

/// One emitter in the scene.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Light {
    pub at: Vec3,
    pub emitter: Emitter,
    pub falloff: Falloff,
    /// Its colour, per channel, as a plain multiplier.
    pub colour: [f64; 3],
    /// Radiant intensity: brightness at unit distance before the falloff curve
    /// is applied.
    pub intensity: f64,
}

/// One draw from an emitter: a place to aim a shadow ray, and what arrives if
/// nothing is in the way.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Sample {
    /// The point on the emitter this sample is about — where the shadow ray
    /// goes.
    pub at: Vec3,
    /// Irradiance arriving per unit of the receiving surface's own cosine, per
    /// channel. The receiver's `n · l` is not in here: this type does not know
    /// what it is lighting.
    pub arriving: [f64; 3],
}

impl Light {
    /// How many samples this emitter needs to be estimated without variance, or
    /// [`None`] when there is no such number.
    ///
    /// A point emitter is `Some(1)`: one shadow ray is not an estimate of its
    /// visibility, it *is* its visibility. A caller running the degenerate mode
    /// asserts on this rather than assuming it, because "the picture is exact"
    /// and "the picture is an average of one sample" look identical from
    /// outside and are not the same claim.
    pub fn exact_in_samples(&self) -> Option<u32> {
        match self.emitter {
            Emitter::Point => Some(1),
            Emitter::Sphere { .. } => None,
        }
    }

    /// Draw one sample of this emitter, as seen from `from`.
    ///
    /// [`None`] when the falloff says nothing arrives — past a windowed
    /// falloff's reach.
    ///
    /// # Why the brightness comes from the centre and only the aim is sampled
    ///
    /// The textbook estimator for an area emitter divides by the density it
    /// sampled with and multiplies by the emitter's own cosine and by `1/d²`;
    /// it is exactly right, and it is exactly right **only with a physical
    /// falloff**. Its near-field behaviour is a cancellation: as a surface
    /// approaches the emitter the emitter's average cosine collapses and the
    /// `1/d²` grows without bound, and the two hold each other up.
    ///
    /// [`Falloff::Windowed`] is not `1/d²` — it is a curve chosen so a torch
    /// has an edge, and it does not grow at all. Put the two together and only
    /// the collapse survives: a wide emitter close to the floor draws a **dark
    /// patch directly beneath itself**, right where it should be brightest.
    /// This is not hypothetical; it is what the first version of this file did,
    /// and the picture is what found it.
    ///
    /// So the two roles are separated instead. Brightness is the emitter's
    /// intensity at its centre through whichever curve the caller chose —
    /// point-source photometry, correct for a small emitter and honest about
    /// what it is for a large one. The emitter's *extent* is used for one thing
    /// only: where to aim the shadow ray, which is what a penumbra is made of.
    /// The two agree with the full estimator wherever an emitter is small
    /// against its distance, which is every scene this crate is pointed at, and
    /// they disagree gracefully rather than catastrophically where it is not.
    ///
    /// The silhouette a sphere presents is a disc facing the receiver, so that
    /// is what is sampled. A receiver *inside* the emitter has no silhouette at
    /// all and falls back to the centre — a scene a caller can build, not an
    /// error, and one where a penumbra is not a meaningful thing to ask for.
    pub fn sample(&self, from: Vec3, stream: &mut Stream) -> Option<Sample> {
        let towards = self.at - from;
        let distance = towards.length();
        let falloff = self.falloff.at(distance)?;
        let at = match self.emitter {
            Emitter::Sphere { radius } if radius < distance => {
                let (u, v) = (towards / distance).orthonormal_basis();
                // Uniform over the disc's *area*: the square root is what keeps
                // the samples from crowding the centre, which would narrow every
                // penumbra by an amount that looks like a smaller emitter.
                let spread = radius * stream.unit().sqrt();
                let azimuth = std::f64::consts::TAU * stream.unit();
                self.at + u * (spread * azimuth.cos()) + v * (spread * azimuth.sin())
            }
            Emitter::Point | Emitter::Sphere { .. } => self.at,
        };
        let scale = self.intensity * falloff;
        Some(Sample {
            at,
            arriving: [
                self.colour[0] * scale,
                self.colour[1] * scale,
                self.colour[2] * scale,
            ],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{Emitter, Falloff, Light};
    use crate::rng::Stream;
    use crate::vector::Vec3;

    fn light(emitter: Emitter) -> Light {
        Light {
            at: Vec3::new(0.0, 0.0, 0.0),
            emitter,
            falloff: Falloff::InverseSquare,
            colour: [1.0, 1.0, 1.0],
            intensity: 1.0,
        }
    }

    #[test]
    fn a_windowed_falloff_has_nothing_at_all_past_its_reach() {
        let falloff = Falloff::Windowed { reach: 6.0 };
        assert_eq!(falloff.at(0.0), Some(1.0), "at the emitter itself");
        assert_eq!(falloff.at(3.0), Some(0.25), "half way out, squared");
        assert_eq!(falloff.at(6.0), None, "exactly at the edge is already out");
        assert_eq!(falloff.at(60.0), None, "and beyond it");
    }

    #[test]
    fn a_point_emitter_is_exact_in_one_sample_and_a_sphere_is_not() {
        assert_eq!(light(Emitter::Point).exact_in_samples(), Some(1));
        assert_eq!(light(Emitter::Sphere { radius: 0.5 }).exact_in_samples(), None);
    }

    #[test]
    fn a_point_emitter_draws_the_same_sample_every_time_whatever_the_stream_says() {
        // What "degenerate" means, stated as a test: the emitter's own shape is
        // the only source of variance in a direct-lighting estimator, so with a
        // point there is none left and the stream is not consulted.
        let point = light(Emitter::Point);
        let from = Vec3::new(3.0, 4.0, 0.0);
        let first = point.sample(from, &mut Stream::new(1, 1)).expect("in reach");
        let second = point.sample(from, &mut Stream::new(999, 77)).expect("in reach");
        assert_eq!(first, second);
        assert_eq!(first.at, point.at, "the sample is the emitter");
        assert_eq!(first.arriving[0], 0.04, "inverse square at five units: 1/25");
    }

    #[test]
    fn a_sphere_lights_a_surface_exactly_as_a_point_does_at_any_distance() {
        // The claim [`Light::sample`]'s doc makes, and the regression that doc
        // is about. The old estimator agreed with a point far away and
        // collapsed to a *dark patch* directly under a wide emitter, because
        // the emitter's own cosine went to zero with no `1/d²` left to hold it
        // up. Sweeping the distance down to the emitter's own radius is what
        // catches that; a single far-field check passed it happily.
        for distance in [40.0, 8.0, 2.0, 1.0, 0.55] {
            let from = Vec3::new(0.0, 0.0, distance);
            let point = light(Emitter::Point)
                .sample(from, &mut Stream::new(0, 0))
                .expect("in reach")
                .arriving[0];
            let sphere = light(Emitter::Sphere { radius: 0.5 });
            let mut stream = Stream::new(5, 9);
            let samples = 20_000;
            let total: f64 = (0..samples)
                .filter_map(|_| sphere.sample(from, &mut stream))
                .map(|sample| sample.arriving[0])
                .sum();
            let mean = total / f64::from(samples);
            assert!(
                (mean - point).abs() < 1e-12,
                "at {distance}: sphere averaged {mean}, point gives {point}"
            );
        }
    }

    #[test]
    fn a_spheres_samples_cover_the_disc_it_shows_the_receiver() {
        // Three ways this goes wrong quietly, all of them showing up as a
        // penumbra of the wrong width and nothing more obvious: samples that
        // leave the disc, samples that crowd its centre (the missing square
        // root), and a disc that is not square to the receiver.
        let sphere = light(Emitter::Sphere { radius: 2.0 });
        let from = Vec3::new(9.0, 0.0, 0.0);
        let towards = (sphere.at - from).normalized();
        let mut stream = Stream::new(4, 4);
        let (draws, mut inner) = (40_000, 0usize);
        for _ in 0..draws {
            let at = sphere.sample(from, &mut stream).expect("in reach").at;
            let offset = at - sphere.at;
            assert!(offset.length() <= 2.0 + 1e-12, "{offset:?} left the disc");
            assert!(
                offset.dot(towards).abs() < 1e-12,
                "{offset:?} is not square to the receiver"
            );
            // Half the disc's area is inside `radius / sqrt(2)`.
            inner += usize::from(offset.length() <= 2.0 / std::f64::consts::SQRT_2);
        }
        let half = f64::from(draws) / 2.0;
        assert!(
            (inner as f64 - half).abs() < 0.02 * half,
            "half the samples should fall in the inner half of the disc's area, {inner} of {draws} did"
        );
    }

    #[test]
    fn a_receiver_inside_the_emitter_falls_back_to_its_centre() {
        // No silhouette to sample, so no penumbra to ask for. It has to be a
        // defined answer rather than a panic: a scene can put a torch inside a
        // wall, and a reference that dies on it stops being able to say what
        // the renderer does there.
        let sphere = light(Emitter::Sphere { radius: 3.0 });
        let sample = sphere
            .sample(Vec3::new(1.0, 0.0, 0.0), &mut Stream::new(1, 2))
            .expect("in reach");
        assert_eq!(sample.at, sphere.at);
    }
}
