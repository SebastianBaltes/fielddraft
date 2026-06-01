//! B-Rep extraction, **Phase 2: iterative surface projection**.
//!
//! Implements the Newton-Raphson surface projection described in
//! `docs/GEOMETRY-KERNEL-CONCEPT.md` ("Phase 2: Iterative
//! Oberflächen-Projektion"). Given a start point near a branch surface,
//! we walk it onto the exact zero level-set of the (logical) field using
//! the analytic gradient from the dual-number evaluator, with
//! **Taubin normalisation**:
//!
//! ```text
//! repeat:
//!     (f, grad) = surface_gradient(body, p)
//!     p ← p - f/|grad|² · grad
//! until |f| < ε
//! ```
//!
//! The `f/|grad|²` step (instead of the textbook `f/|grad|`) is the
//! Taubin correction: it turns the update into the exact first-order
//! Newton step for a locally-linear field and so corrects for
//! non-euclidean (warped / non-normalised) fields on the fly. For a true
//! euclidean SDF (`|grad| = 1`) one step is exact; for warped fields it
//! converges in 2–4 iterations.

use crate::brep::octree::surface_gradient;
use crate::bvh::BoundedCsg;

/// A point that has been projected onto a branch surface, together with
/// the outward-pointing unit surface normal at that point (the normalised
/// gradient `∇f/|∇f|`, which spans the exact tangent plane).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProjectedPoint {
    pub position: [f32; 3],
    pub normal: [f32; 3],
}

/// Squared-gradient floor below which the field is treated as locally
/// flat / degenerate and projection bails out. Well below any healthy
/// `|∇f|² ≈ 1` while comfortably above f32 round-off noise.
const TINY_GRAD_SQ: f32 = 1e-12;

/// Newton-Raphson project `start` onto the zero level-set of `body` using
/// Taubin-normalised steps.
///
/// Returns the converged [`ProjectedPoint`] (position on the surface +
/// unit normal there), or `None` when
///
/// - the gradient degenerates (`|∇f|² < TINY_GRAD_SQ`) at some iterate, or
/// - the iteration does not reach `|f| < eps` within `max_iter` steps.
///
/// `eps` is the surface-distance tolerance (e.g. `1e-6` in f32); `max_iter`
/// caps the walk (e.g. `16`).
pub fn project_to_surface(
    body: &BoundedCsg,
    start: [f32; 3],
    eps: f32,
    max_iter: usize,
) -> Option<ProjectedPoint> {
    let mut p = start;
    // Last gradient with a healthy magnitude. The box (and other
    // min/max-composed) SDFs return a *zero* gradient *exactly* on a face
    // — the C⁰ kink in the field collapses the dual-number derivative
    // there. The convergence step lands precisely on such a face, so the
    // normal must come from the last off-surface iterate, where the
    // gradient is the clean face normal.
    let mut last_good_normal: Option<[f32; 3]> = None;

    for _ in 0..max_iter {
        let (f, grad) = surface_gradient(body, p);
        let grad_sq = grad[0] * grad[0] + grad[1] * grad[1] + grad[2] * grad[2];
        let grad_ok = grad_sq >= TINY_GRAD_SQ;

        if grad_ok {
            let inv_len = 1.0 / grad_sq.sqrt();
            last_good_normal = Some([grad[0] * inv_len, grad[1] * inv_len, grad[2] * inv_len]);
        }

        // Converged: report the surface point. Prefer the gradient here,
        // else fall back to the last healthy normal.
        if f.abs() < eps {
            return last_good_normal.map(|normal| ProjectedPoint { position: p, normal });
        }

        // Not converged and no surface direction to step along → give up.
        if !grad_ok {
            return None;
        }

        // Taubin-normalised Newton step: p -= f/|grad|² · grad.
        let scale = f / grad_sq;
        p = [
            p[0] - scale * grad[0],
            p[1] - scale * grad[1],
            p[2] - scale * grad[2],
        ];
    }

    // Hit the iteration cap: accept only if the final iterate is on-surface.
    let (f, _) = surface_gradient(body, p);
    if f.abs() < eps {
        last_good_normal.map(|normal| ProjectedPoint { position: p, normal })
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::csg::CsgNode;
    use fielddraft_protocol::{FeatureId, Primitive};

    /// Axis-aligned box centred at the origin with the given half-extents.
    fn box_body(half: [f32; 3]) -> BoundedCsg {
        let csg = CsgNode::Leaf {
            id: FeatureId::from_u32(1),
            primitive: Primitive::Box {
                center: [0.0, 0.0, 0.0],
                half_extents: half,
            },
        };
        BoundedCsg::from_csg(&csg)
    }

    fn sphere_body(radius: f32) -> BoundedCsg {
        let csg = CsgNode::Leaf {
            id: FeatureId::from_u32(1),
            primitive: Primitive::Sphere {
                center: [0.0, 0.0, 0.0],
                radius,
            },
        };
        BoundedCsg::from_csg(&csg)
    }

    #[test]
    fn projects_onto_box_face() {
        let body = box_body([1.0, 1.0, 1.0]);
        // A point just outside the +X face, near its centre.
        let proj = project_to_surface(&body, [1.2, 0.1, -0.2], 1e-6, 16)
            .expect("should converge onto the +X face");

        let (f, _) = surface_gradient(&body, proj.position);
        assert!(f.abs() < 1e-5, "projected point not on surface: f = {f}");

        // The +X face has outward normal ~(+1, 0, 0).
        assert!(proj.normal[0] > 0.99, "normal not +X: {:?}", proj.normal);
        assert!(proj.normal[1].abs() < 1e-3 && proj.normal[2].abs() < 1e-3);
        // It must land on the face plane x ≈ +1.
        assert!((proj.position[0] - 1.0).abs() < 1e-4, "x = {}", proj.position[0]);
    }

    #[test]
    fn projects_onto_box_face_from_inside() {
        let body = box_body([1.0, 1.0, 1.0]);
        // Inside the box, closest to the -Y face.
        let proj = project_to_surface(&body, [0.1, -0.85, 0.2], 1e-6, 16)
            .expect("should converge onto the -Y face");
        assert!(proj.normal[1] < -0.99, "normal not -Y: {:?}", proj.normal);
        assert!((proj.position[1] + 1.0).abs() < 1e-4, "y = {}", proj.position[1]);
    }

    #[test]
    fn projects_onto_sphere() {
        let body = sphere_body(1.0);
        let proj = project_to_surface(&body, [0.3, 0.4, 0.9], 1e-6, 16)
            .expect("should converge onto the sphere");
        let r = (proj.position[0] * proj.position[0]
            + proj.position[1] * proj.position[1]
            + proj.position[2] * proj.position[2])
            .sqrt();
        assert!((r - 1.0).abs() < 1e-4, "radius = {r}");
        // On a sphere the outward normal is the radial direction.
        let dot = proj.normal[0] * proj.position[0]
            + proj.normal[1] * proj.position[1]
            + proj.normal[2] * proj.position[2];
        assert!(dot > 0.99, "normal not radial: {:?}", proj.normal);
    }

    #[test]
    fn returns_some_with_unit_normal() {
        let body = sphere_body(2.0);
        let proj = project_to_surface(&body, [2.5, 0.0, 0.0], 1e-6, 16).unwrap();
        let n = proj.normal;
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        assert!((len - 1.0).abs() < 1e-5, "normal not unit length: {len}");
    }
}
