//! What is in the world, and the two questions a tracer asks it.
//!
//! # The units are the caller's, and they have to be isotropic
//!
//! Nothing here converts anything. A box's corners and a light's position are
//! in whatever unit the caller measures the world in, and this crate treats
//! that unit as the same on all three axes.
//!
//! That is a real constraint and it is easy to miss, because the *first* thing
//! a tracer does is unaffected by it: whether a segment meets a box survives
//! any affine change of coordinates, so a visibility-only render is correct
//! even in squashed axes. Everything after that is not. A cosine, a solid
//! angle, a distance in a falloff curve and the shape of a penumbra all read
//! the metric directly, so a caller whose world is authored with `z` in some
//! other unit — as this project's is, eleven of them to a tile — has to scale
//! it on the way in, and scale it back on the way out if it wants to talk about
//! its own coordinates again.
//!
//! Stated here rather than absorbed, because absorbing it is how a reference
//! quietly becomes a picture of a differently-shaped world than the one it is
//! being compared to.

use crate::aabb::Aabb;
use crate::vector::Vec3;

/// How far a ray is pushed off the surface it starts on before it is asked
/// what it can see.
///
/// A shadow ray leaving a surface starts exactly on it, and the surface it
/// starts on is in the scene, so without an offset every point shadows itself
/// wherever rounding puts the crossing at a hair above zero rather than a hair
/// below. Along the normal and not along the ray: an offset along the ray does
/// nothing at all at a grazing angle, which is the exact case that needs it.
///
/// The size is a compromise with no tuning in it — large enough that `f64`
/// rounding at world magnitudes in the hundreds cannot cross it, small enough
/// to be far under any geometric feature (a hundred-millionth of a tile here).
/// A reference whose answer moves when this moves is not a reference, so
/// [`Scene`]'s own tests sweep it across four orders of magnitude and assert
/// the picture does not change.
pub const SURFACE_BIAS: f64 = 1e-8;

/// One solid thing.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Body {
    pub shape: Aabb,
    /// The fraction of each channel it reflects. Diffuse throughout — this
    /// crate has no specular term, because nothing it is checking has one.
    pub albedo: [f64; 3],
}

/// An infinite horizontal plane, at a height.
///
/// Infinite because the alternative is a large box, and a large box has edges
/// that a grazing shadow ray can find. A plane the scene is standing on is not
/// a thing the renderer being checked has an edge for either.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Ground {
    pub z: f64,
    pub albedo: [f64; 3],
}

/// Which surface a ray found.
///
/// Carried out to the caller so it can ask the renderer the same question — a
/// pixel where the two disagree about *what is there* is a depth-sorting
/// difference, and counting it as a lighting difference would file it under the
/// wrong defect entirely.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Surface {
    Body(usize),
    Ground,
}

/// Where a ray met the world.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Hit {
    pub surface: Surface,
    /// How far along the ray, in multiples of its direction vector.
    pub t: f64,
    pub at: Vec3,
    /// The outward normal of the face that was hit — facing the ray.
    pub normal: Vec3,
    pub albedo: [f64; 3],
}

/// Everything a ray can meet.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct Scene {
    pub bodies: Vec<Body>,
    pub ground: Option<Ground>,
}

impl Scene {
    /// The nearest surface along `from + t * direction` with `t > after`.
    ///
    /// `after` is how a caller says where the ray begins: [`f64::NEG_INFINITY`]
    /// for a camera ray, whose origin is an arbitrary point on an infinite
    /// line, and `0.0` for a ray leaving a surface — which is also why
    /// [`SURFACE_BIAS`] exists, since "leaving a surface" and "starting on one"
    /// are the same numbers.
    pub fn hit(&self, from: Vec3, direction: Vec3, after: f64) -> Option<Hit> {
        let mut best: Option<Hit> = None;
        let mut keep = |hit: Hit| {
            if hit.t > after && best.is_none_or(|previous| hit.t < previous.t) {
                best = Some(hit);
            }
        };
        for (index, body) in self.bodies.iter().enumerate() {
            let Some(crossing) = body.shape.crossing(from, direction) else {
                continue;
            };
            // Both ends are candidates. A ray leaving a box it started inside
            // meets it at `far`, and taking only `near` would let a bounce
            // escape through a wall it is standing in.
            for (t, normal) in [
                (crossing.near, crossing.entry_normal(direction)),
                (crossing.far, crossing.exit_normal(direction)),
            ] {
                keep(Hit {
                    surface: Surface::Body(index),
                    t,
                    at: from + direction * t,
                    normal,
                    albedo: body.albedo,
                });
            }
        }
        if let Some(ground) = self.ground {
            // A ray parallel to the plane never meets it. It is the same rule
            // the box slab uses for a zero-thickness axis, and for the same
            // reason: a plane is crossed, never travelled along.
            if direction.z != 0.0 {
                let t = (ground.z - from.z) / direction.z;
                keep(Hit {
                    surface: Surface::Ground,
                    t,
                    at: from + direction * t,
                    normal: match direction.z < 0.0 {
                        true => Vec3::new(0.0, 0.0, 1.0),
                        false => Vec3::new(0.0, 0.0, -1.0),
                    },
                    albedo: ground.albedo,
                });
            }
        }
        best
    }

    /// Whether anything stands strictly between `from` and `to`.
    ///
    /// The open segment: a surface exactly at either end does not block, which
    /// is what makes a point on a box not shadow itself and a sample on an
    /// emitter's own surface not shadow the emitter. `from` is expected to have
    /// been pushed off its surface by [`SURFACE_BIAS`] already — this function
    /// cannot do it, because it does not know which surface the caller was on.
    ///
    /// `except` names a surface that does not count as an occluder. [`None`] —
    /// nothing is exempt — is the physical answer and what every physical mode
    /// asks for: a body between a point and a light stops the light whether or
    /// not the point is standing on that same body. A caller passes [`Some`]
    /// only to compute a *model* that has no normals and therefore cannot tell
    /// a surface's own back from an obstruction — see
    /// [`crate::trace::Brdf::Flat`], which is the only thing in this crate that
    /// does.
    ///
    /// Meeting a box at a single point — a corner, a grazed edge — is *not*
    /// blocking. Zero thickness of occluder is not an occlusion, and calling it
    /// one would put a hard shadow line along every silhouette edge in the
    /// scene. [`Aabb::crossing`](crate::aabb::Aabb::crossing) has already
    /// discarded the grazing cases that have length but no interior.
    ///
    /// A zero-length segment is not blocked by anything: there is nothing
    /// between a point and itself. It arises where an emitter sample lands
    /// exactly on the surface being lit, which is a scene a caller may build
    /// and not an error.
    pub fn blocked(&self, from: Vec3, to: Vec3, except: Option<Surface>) -> bool {
        let segment = to - from;
        if segment == Vec3::ZERO {
            return false;
        }
        let stands_in_the_way = |crossing: Option<crate::aabb::Crossing>| match crossing {
            Some(crossing) => crossing.far > 0.0 && crossing.near < 1.0 && crossing.far > crossing.near,
            None => false,
        };
        if self
            .bodies
            .iter()
            .enumerate()
            .filter(|(index, _)| except != Some(Surface::Body(*index)))
            .any(|(_, body)| stands_in_the_way(body.shape.crossing(from, segment)))
        {
            return true;
        }
        // The ground is an occluder like any other: a light below the floor is
        // behind it. As a plane it is crossed rather than entered, so the
        // interval test above cannot be reused — a crossing of zero length is
        // exactly what a plane produces, and here it does block.
        match self.ground {
            Some(ground) if segment.z != 0.0 && except != Some(Surface::Ground) => {
                let t = (ground.z - from.z) / segment.z;
                t > 0.0 && t < 1.0
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Body, Ground, Scene, Surface};
    use crate::aabb::Aabb;
    use crate::vector::Vec3;

    fn one_box() -> Scene {
        Scene {
            bodies: vec![Body {
                shape: Aabb::between(Vec3::new(-0.5, -0.5, 0.0), Vec3::new(0.5, 0.5, 2.0)),
                albedo: [0.8; 3],
            }],
            ground: Some(Ground {
                z: 0.0,
                albedo: [0.5; 3],
            }),
        }
    }

    #[test]
    fn a_camera_ray_finds_the_nearest_surface_from_anywhere_on_its_own_line() {
        // The property a camera ray needs and a shadow ray does not: the origin
        // is an arbitrary point of an infinite line, so the answer must not
        // depend on where along it the caller happened to hand it over.
        let scene = one_box();
        let direction = Vec3::new(0.0, 0.0, -1.0);
        let straight_down = |from: Vec3| {
            scene
                .hit(from, direction, f64::NEG_INFINITY)
                .expect("the column meets the box")
        };
        let high = straight_down(Vec3::new(0.0, 0.0, 900.0));
        let low = straight_down(Vec3::new(0.0, 0.0, -900.0));
        assert_eq!(high.surface, Surface::Body(0));
        assert_eq!(low.surface, Surface::Body(0), "same line, same nearest surface");
        assert_eq!(high.at.z, 2.0, "the lid");
        assert_eq!(low.at.z, 2.0, "and the same lid from below the world");
        assert_eq!(high.normal, Vec3::new(0.0, 0.0, 1.0), "facing the ray");
    }

    #[test]
    fn the_ground_is_hit_where_no_body_stands() {
        let scene = one_box();
        let hit = scene
            .hit(
                Vec3::new(4.0, 4.0, 10.0),
                Vec3::new(0.0, 0.0, -1.0),
                f64::NEG_INFINITY,
            )
            .expect("the floor is infinite");
        assert_eq!(hit.surface, Surface::Ground);
        assert_eq!(hit.at.z, 0.0);
    }

    #[test]
    fn a_box_between_a_point_and_a_light_blocks_it_and_one_beside_it_does_not() {
        let scene = one_box();
        let light = Vec3::new(0.0, -6.0, 1.0);
        assert!(
            scene.blocked(Vec3::new(0.0, 3.0, 1.0), light, None),
            "straight through the box"
        );
        assert!(
            !scene.blocked(Vec3::new(4.0, 3.0, 1.0), light, None),
            "well to the side of it"
        );
    }

    #[test]
    fn an_exempt_body_does_not_occlude_and_its_neighbour_still_does() {
        // What [`crate::trace::Brdf::Flat`] asks for, and the two halves of it
        // that have to hold at once: the named body stops counting, and nothing
        // else changes. An exemption that let the *whole* scene through would
        // pass any test written against one box, and would silently turn the
        // reference into a picture with no shadows in it at all.
        let mut scene = one_box();
        scene.bodies.push(Body {
            shape: Aabb::between(Vec3::new(1.5, -0.5, 0.0), Vec3::new(2.5, 0.5, 2.0)),
            albedo: [0.8; 3],
        });
        let light = Vec3::new(6.0, 0.0, 1.0);
        // A point on the first box's own east face, with the light beyond the
        // second box: the first body is between it and the light because the
        // point is *on* it, and the second is between it and the light because
        // it stands there.
        let from = Vec3::new(0.5, 0.0, 1.0);
        assert!(scene.blocked(from, light, None), "both bodies are in the way");
        assert!(
            scene.blocked(from, light, Some(Surface::Body(0))),
            "its own body is exempt, the one standing in the way is not"
        );
        assert!(
            !scene.blocked(from, light, Some(Surface::Body(1))),
            "with the standing body exempt, only the surface's own remains"
        );
    }

    #[test]
    fn an_exempt_ground_plane_stops_blocking_a_light_below_it() {
        // The same rule for the surface that is not a body. A model with no
        // normals lights the ground from a lamp in the cellar, and a reference
        // computing that model has to be able to say so — see
        // [`crate::trace::Brdf::Flat`].
        let scene = one_box();
        let (from, cellar) = (Vec3::new(3.0, 3.0, 1.0), Vec3::new(3.0, 3.0, -5.0));
        assert!(scene.blocked(from, cellar, None), "the floor is between them");
        assert!(
            !scene.blocked(from, cellar, Some(Surface::Ground)),
            "unless it is exempt"
        );
    }

    #[test]
    fn a_point_on_a_box_does_not_shadow_itself_at_any_plausible_bias() {
        // The claim [`SURFACE_BIAS`]'s doc makes, as a test across four orders
        // of magnitude. A reference whose shadows move when this constant moves
        // is a reference with a tuned parameter in it, and the number it
        // produces would be about the parameter rather than about the scene.
        let scene = one_box();
        let light = Vec3::new(0.0, -6.0, 6.0);
        // A point on the box's own south face, and one on its lid: both are on
        // a surface of the very body that would shadow them.
        for (point, normal, why) in [
            (
                Vec3::new(0.0, -0.5, 1.0),
                Vec3::new(0.0, -1.0, 0.0),
                "the south face",
            ),
            (Vec3::new(0.0, 0.0, 2.0), Vec3::new(0.0, 0.0, 1.0), "the lid"),
        ] {
            for bias in [1e-10, 1e-8, 1e-6, 1e-4] {
                assert!(
                    !scene.blocked(point + normal * bias, light, None),
                    "{why} shadowed itself at bias {bias}"
                );
            }
        }
    }

    #[test]
    fn a_ray_grazing_a_silhouette_edge_is_not_blocked_by_it() {
        // Zero thickness of occluder is not an occlusion. Otherwise every
        // silhouette in the scene grows a hard dark line one ray wide, which
        // reads as a real shadow and is not one.
        let scene = one_box();
        // Exactly along the `x = 0.5` face, at the height of the lid: the
        // segment touches the box's edge for its whole length without ever
        // being inside it.
        assert!(
            !scene.blocked(Vec3::new(0.5, 3.0, 2.0), Vec3::new(0.5, -6.0, 2.0), None),
            "running along an edge is not passing through the body"
        );
    }

    #[test]
    fn the_floor_blocks_a_light_underneath_it() {
        let scene = one_box();
        assert!(
            scene.blocked(Vec3::new(3.0, 3.0, 1.0), Vec3::new(3.0, 3.0, -5.0), None),
            "the ground plane is an occluder like any other"
        );
    }

    #[test]
    fn a_ray_leaving_a_box_it_started_inside_meets_that_boxs_far_side() {
        // Otherwise a bounce inside a body escapes through its own wall and the
        // interior lights up, which looks like a light leak in the renderer
        // being checked rather than in the reference.
        let scene = one_box();
        let hit = scene
            .hit(Vec3::new(0.0, 0.0, 1.0), Vec3::new(1.0, 0.0, 0.0), 0.0)
            .expect("the wall is still there from inside");
        assert_eq!(hit.surface, Surface::Body(0));
        assert_eq!(hit.at.x, 0.5, "the far wall");
        assert_eq!(hit.normal, Vec3::new(-1.0, 0.0, 0.0), "facing back at the ray");
    }
}
