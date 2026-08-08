//! A set of honest faces standing in for a sprite's picture, for whatever
//! needs the shape a flat billboard cannot give it.
//!
//! `docs/gbuffer.md` step 4c's own abstraction, and deliberately not shaped
//! around any one producer: [`crate::facing::Prism`] is the first (its
//! treads' tops and risers today), and nothing in this module knows that — a
//! sloped roof, or any future custom geometry, builds its own [`Mesh`] the
//! same way, and whatever walks one draws every [`Face`] alike. A [`Face`] is
//! per-*sprite*, not per-tile: several sprites can stand on one tile, and
//! each carries its own mesh or none.

use crate::camera::WorldSpot;

/// How many corners one [`Face`] can hold.
///
/// A cap on the measurement, not an aspiration — the same discipline
/// [`crate::facing::MAX_TREADS`] states for itself. Every face built today is
/// a rectangle (a tread's top strip, or the plane between two treads); a
/// producer that needs a fifth corner raises this then, against its own
/// shape, rather than this module guessing one in advance.
pub const MAX_FACE_VERTICES: usize = 4;

/// How many faces one [`Mesh`] can hold.
///
/// Two per tread — a top and a riser — so this is
/// [`crate::facing::MAX_TREADS`]'s own cap, doubled. The same discipline as
/// [`MAX_FACE_VERTICES`]: a cap on what [`crate::facing::Prism::mesh`]
/// measurably produces.
pub const MAX_MESH_FACES: usize = 2 * crate::facing::MAX_TREADS as usize;

/// One flat, convex polygon, with its own honest normal.
///
/// Not blended, not derived from a neighbour, not an approximation standing
/// in for a shape it is not — `docs/gbuffer.md` decision 3's whole argument.
/// A tread's top and its riser are two different [`Face`]s, each carrying the
/// normal that is actually true of it, rather than one surface reading like
/// both by a formula.
#[derive(Clone, Copy, Debug)]
pub struct Face {
    vertices: [WorldSpot; MAX_FACE_VERTICES],
    count: u8,
    /// The unit normal this face's light — and its projected geometry, once
    /// something reads one — both answer with.
    pub normal: [f32; 3],
}

impl Face {
    /// A face from its corners, in ring order, and its own normal.
    ///
    /// `None` if `corners` is empty or longer than [`MAX_FACE_VERTICES`] —
    /// the same shape [`crate::facing::Prism::new`] refuses an out-of-range
    /// profile with.
    pub fn new(corners: &[WorldSpot], normal: [f32; 3]) -> Option<Self> {
        if corners.is_empty() || corners.len() > MAX_FACE_VERTICES {
            return None;
        }
        let mut vertices = [WorldSpot::default(); MAX_FACE_VERTICES];
        vertices[..corners.len()].copy_from_slice(corners);
        Some(Self {
            vertices,
            count: corners.len() as u8,
            normal,
        })
    }

    /// Its corners, in ring order.
    pub fn vertices(&self) -> &[WorldSpot] {
        &self.vertices[..usize::from(self.count)]
    }

    /// This face's corners, fan-triangulated into two triangles: `0,1,2,0,2,3`.
    ///
    /// Every [`Face`] built anywhere today has exactly four corners (see
    /// [`crate::facing::Prism::mesh`]), which this assumes rather than folds
    /// into a general polygon fan — a producer with three or five corners
    /// widens this the day it exists, against its own shape, the same
    /// discipline [`MAX_FACE_VERTICES`] states for itself. `crate::solids`
    /// inlines the identical split on already-*projected* screen corners
    /// (`solids.rs`'s own box faces); this operates one stage earlier, on
    /// world corners, because `docs/gbuffer.md` step 4c's render pass needs
    /// both a corner's screen position and its true world position per
    /// vertex, and only the latter is available at this stage.
    pub fn fan(&self) -> [WorldSpot; 6] {
        let v = self.vertices();
        assert_eq!(v.len(), 4, "only the four-corner case is triangulated today");
        [v[0], v[1], v[2], v[0], v[2], v[3]]
    }
}

/// A list of [`Face`]s belonging to one sprite.
///
/// Fixed capacity rather than a `Vec` — [`crate::facing::Prism`]'s own doc
/// gives the reason, and it applies here for the same load: a mesh is built
/// fresh from a placed sprite's own facts wherever it is collected, which
/// today is every visible climbable static, every frame.
#[derive(Clone, Copy, Debug)]
pub struct Mesh {
    faces: [Face; MAX_MESH_FACES],
    count: u8,
}

impl Mesh {
    /// A mesh with no faces yet.
    pub const EMPTY: Self = Self {
        // Never read past `count`, so the placeholder's own shape does not
        // matter — `Face::new` is not `const`, so this cannot call it, and a
        // face manufactured by hand here would be a second way to build one.
        faces: [Face {
            vertices: [WorldSpot {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            }; MAX_FACE_VERTICES],
            count: 0,
            normal: [0.0, 0.0, 1.0],
        }; MAX_MESH_FACES],
        count: 0,
    };

    /// Add one more face.
    ///
    /// Public: this module's own doc says any producer builds a [`Mesh`] the
    /// same way [`crate::facing::Prism`] does, and a producer outside this
    /// crate is only that same claim taken literally.
    ///
    /// Panics past [`MAX_MESH_FACES`] — every producer inside this crate today
    /// pushes at most that many by construction ([`crate::facing::Prism::mesh`]
    /// pushes two per tread, capped at [`crate::facing::MAX_TREADS`]), so
    /// reaching the limit there would mean a producer's own cap and this one
    /// disagree, a bug in the producer to find rather than paper over with a
    /// dropped face. An outside caller takes on the same contract.
    pub fn push(&mut self, face: Face) {
        self.faces[usize::from(self.count)] = face;
        self.count += 1;
    }

    /// Its faces.
    pub fn faces(&self) -> &[Face] {
        &self.faces[..usize::from(self.count)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spot(x: f64, y: f64, z: f64) -> WorldSpot {
        WorldSpot { x, y, z }
    }

    #[test]
    fn a_face_keeps_its_corners_and_its_normal() {
        let corners = [
            spot(0.0, 0.0, 0.0),
            spot(1.0, 0.0, 0.0),
            spot(1.0, 1.0, 0.0),
            spot(0.0, 1.0, 0.0),
        ];
        let face = Face::new(&corners, [0.0, 0.0, 1.0]).unwrap();
        assert_eq!(face.vertices(), &corners);
        assert_eq!(face.normal, [0.0, 0.0, 1.0]);
    }

    #[test]
    fn a_face_fans_into_two_triangles_sharing_the_first_corner() {
        let corners = [
            spot(0.0, 0.0, 0.0),
            spot(1.0, 0.0, 0.0),
            spot(1.0, 1.0, 0.0),
            spot(0.0, 1.0, 0.0),
        ];
        let face = Face::new(&corners, [0.0, 0.0, 1.0]).unwrap();
        assert_eq!(
            face.fan(),
            [
                corners[0], corners[1], corners[2], corners[0], corners[2], corners[3]
            ],
            "0,1,2 and 0,2,3 — both triangles share the ring's first corner"
        );
    }

    #[test]
    fn a_face_refuses_an_empty_or_oversized_ring() {
        assert!(Face::new(&[], [0.0, 0.0, 1.0]).is_none());
        let too_many = [spot(0.0, 0.0, 0.0); MAX_FACE_VERTICES + 1];
        assert!(Face::new(&too_many, [0.0, 0.0, 1.0]).is_none());
    }

    #[test]
    fn an_empty_mesh_has_no_faces() {
        assert!(Mesh::EMPTY.faces().is_empty());
    }

    #[test]
    fn pushed_faces_come_back_in_order() {
        let mut mesh = Mesh::EMPTY;
        let top = Face::new(&[spot(0.0, 0.0, 5.0); 4], [0.0, 0.0, 1.0]).unwrap();
        let riser = Face::new(&[spot(0.0, 0.0, 0.0); 4], [1.0, 0.0, 0.0]).unwrap();
        mesh.push(top);
        mesh.push(riser);
        let faces = mesh.faces();
        assert_eq!(faces.len(), 2);
        assert_eq!(faces[0].normal, [0.0, 0.0, 1.0]);
        assert_eq!(faces[1].normal, [1.0, 0.0, 0.0]);
    }
}
