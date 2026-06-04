//! General numeric SDF edge tracer (DL-68).
//!
//! An edge is the joint zero-set of two per-face scalar fields,
//! `E = { p : f_a(p) = 0 ∧ f_b(p) = 0 }`. Two evaluable fields with usable
//! gradients are all this needs — it never asks whether a face is a plane, a
//! sphere, a swept Bézier or a domain-warp. That is the whole point: it
//! replaces DL-67's per-pair closed-form circle (and its slide toward "1000
//! analytic special cases") with **one** method that generalises to any face
//! the SDF can evaluate.
//!
//! Two building blocks:
//!   1. **Project** ([`project_to_edge`]) — Newton on `(f_a, f_b) = (0, 0)`
//!      pulls a near-edge sample *exactly* onto the edge (quadratic
//!      convergence, independent of where the sample sat). Cures jaggedness.
//!   2. **Trace** (DL-68 S3, next) — march along the edge tangent
//!      `t = ∇f_a × ∇f_b`, reprojecting each step, until a bounding vertex or a
//!      closed loop. Cures fragmentation / missing pieces.
//!
//! Everything runs in **f64**: the dual SDF layer is f64 internally, and the
//! rank guard near tangential intersections lives well below f32-ε, so the f32
//! render/mesh floor would cap the achievable exactness. See
//! [`crate::sdf_dual::halfspace_value_grad`] / [`crate::sdf_dual::bounded_value_grad`].
//!
//! This is the edge (1D) analogue of DL-66's [`super::edge_walker`]
//! `refine_triple_vertex` (a 0D vertex = joint zero-set of *three* fields).

// Transitional: `Field` / `project_to_edge` / `edge_tangent` are exercised by
// this module's tests now and wired into `build_wireframe` in DL-68 S3/S4.
// Remove this allow once the tracer replaces `fit_analytic_circle`.
#![allow(dead_code)]

use fielddraft_protocol::{HalfSpace, Transform};

use super::vec3::{add, cross, dot, norm, scale};
use crate::bvh::BoundedCsg;
use crate::csg::EvalMode;
use crate::sdf_dual::{bounded_value_grad, halfspace_value_grad};

/// One per-face scalar field whose zero set *is* that face, together with a
/// usable gradient. The edge tracer is generic over which kind backs a face:
/// analytic primitives get the exact, globally smooth half-space; everything
/// else (Extrude/Revolve/sweep/warp) gets its leaf SDF via dual numbers.
pub(crate) enum Field<'a> {
    /// Analytic per-face half-space (box face, sphere, infinite cylinder, cone,
    /// torus) in its centred local frame, plus the local→world transform.
    /// Globally smooth — the preferred field wherever it exists, because the
    /// leaf SDF of a multi-face primitive (e.g. a box) is only C0 at the
    /// primitive's own edges.
    Analytic { hs: HalfSpace, xf: Transform },
    /// General fallback: a single leaf subtree evaluated by the dual SDF layer.
    /// Its zero set is the whole primitive boundary; near a seam, away from the
    /// primitive's own creases, it is smooth and its gradient is the surface
    /// normal — enough for project + trace on swept/warped faces.
    Leaf { node: &'a BoundedCsg },
}

impl Field<'_> {
    /// Field value and gradient at a world-space point, in f64.
    #[inline]
    pub(crate) fn value_grad(&self, p: [f64; 3]) -> (f64, [f64; 3]) {
        match self {
            Field::Analytic { hs, xf } => halfspace_value_grad(hs, xf, p),
            // A single leaf has no smooth blend, so Logical == Visual; Logical
            // is the well-defined hard branch the B-Rep pipeline extracts in.
            Field::Leaf { node } => bounded_value_grad(node, p, EvalMode::Logical),
        }
    }
}

/// Newton iteration cap. With quadratic convergence 8 is generous; a sample
/// that has not converged by then sits on a degenerate/spurious pair.
const PROJECT_MAX_ITER: usize = 8;
/// Convergence tolerance on `max(|f_a|, |f_b|)` (world units). The fields are
/// true distances near their surfaces, so this is a metric tolerance.
const PROJECT_TOL: f64 = 1e-9;
/// Rank guard: reject the step when `sin²θ` between the two surface normals
/// drops below this (θ ≲ 0.06°). There the cross product collapses, the
/// Jacobian is rank-deficient, and the "edge" is a tangential degeneracy with
/// no isolated curve — mirrors `refine_triple_vertex`'s eigenvalue guard one
/// dimension up.
const RANK_SIN2_MIN: f64 = 1e-6;

/// Project a near-edge point exactly onto `{f_a = 0, f_b = 0}` by minimum-norm
/// Newton.
///
/// The residual is `F = [f_a; f_b]` with Jacobian `J = [∇f_a; ∇f_b]` (2×3).
/// `J` is wide, so the minimum-norm Newton step is
/// `Δ = Jᵀ (J Jᵀ)⁻¹ (−F)`: it moves `p` only within the plane spanned by the
/// two normals (driving both distances linearly to zero) and leaves the edge
/// tangent untouched. `J Jᵀ` is the 2×2 Gram matrix of the gradients, whose
/// determinant is exactly `|∇f_a × ∇f_b|²` — the squared edge-tangent length —
/// so the rank guard and the tangent share one quantity.
///
/// Returns `None` when the pair is tangential/rank-deficient at `p` or the
/// iteration fails to converge (a spurious sample that is not actually near a
/// shared zero).
pub(crate) fn project_to_edge(fa: &Field, fb: &Field, p0: [f64; 3]) -> Option<[f64; 3]> {
    let mut p = p0;
    for _ in 0..PROJECT_MAX_ITER {
        let (va, ga) = fa.value_grad(p);
        let (vb, gb) = fb.value_grad(p);
        if va.abs().max(vb.abs()) < PROJECT_TOL {
            return Some(p);
        }
        if !va.is_finite() || !vb.is_finite() {
            return None;
        }

        // 2×2 Gram matrix M = J Jᵀ.
        let m00 = dot(ga, ga);
        let m01 = dot(ga, gb);
        let m11 = dot(gb, gb);
        let det = m00 * m11 - m01 * m01; // = |ga × gb|²

        // Scale-invariant rank guard: sin²θ = det / (|ga|²|gb|²).
        let scale_sq = m00 * m11;
        if scale_sq <= 0.0 || det <= RANK_SIN2_MIN * scale_sq {
            return None;
        }

        // y = M⁻¹ (−F); Δ = Jᵀ y = ga·y0 + gb·y1.
        let f0 = -va;
        let f1 = -vb;
        let inv = 1.0 / det;
        let y0 = inv * (m11 * f0 - m01 * f1);
        let y1 = inv * (-m01 * f0 + m00 * f1);
        let delta = add(scale(ga, y0), scale(gb, y1));
        p = add(p, delta);

        if !p[0].is_finite() || !p[1].is_finite() || !p[2].is_finite() {
            return None;
        }
    }
    // Final acceptance check after the last step.
    let (va, _) = fa.value_grad(p);
    let (vb, _) = fb.value_grad(p);
    (va.abs().max(vb.abs()) < PROJECT_TOL).then_some(p)
}

/// Unit edge tangent at a point assumed to lie on the edge: `∇f_a × ∇f_b`,
/// normalised. Returns `None` at a tangential degeneracy (collapsing cross
/// product) — the same condition the projection's rank guard rejects.
#[allow(dead_code)] // consumed by the DL-68 S3 tracer (next step)
pub(crate) fn edge_tangent(fa: &Field, fb: &Field, p: [f64; 3]) -> Option<[f64; 3]> {
    let (_, ga) = fa.value_grad(p);
    let (_, gb) = fb.value_grad(p);
    let t = cross(ga, gb);
    let len = norm(t);
    let scale_len = norm(ga) * norm(gb);
    if scale_len <= 0.0 || len * len <= RANK_SIN2_MIN * scale_len * scale_len {
        return None;
    }
    Some(scale(t, 1.0 / len))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Plane half-space `n·p = offset`, with `n` normalised, in world coords.
    fn plane(normal: [f64; 3], offset: f64) -> Field<'static> {
        let n = norm(normal);
        Field::Analytic {
            hs: HalfSpace::Plane {
                normal: [
                    (normal[0] / n) as f32,
                    (normal[1] / n) as f32,
                    (normal[2] / n) as f32,
                ],
                offset: (offset / n) as f32,
            },
            xf: Transform::IDENTITY,
        }
    }

    /// Sphere of `radius` centred at `center` (centre folded into the
    /// transform translate, mirroring `branch_halfspace_map`).
    fn sphere(center: [f64; 3], radius: f64) -> Field<'static> {
        Field::Analytic {
            hs: HalfSpace::Sphere { radius: radius as f32 },
            xf: Transform {
                translate: [center[0] as f32, center[1] as f32, center[2] as f32],
                ..Transform::IDENTITY
            },
        }
    }

    /// Infinite cylinder of `radius` about the Y axis through `center`.
    fn cylinder_y(center: [f64; 3], radius: f64) -> Field<'static> {
        Field::Analytic {
            hs: HalfSpace::InfiniteCylinder { radius: radius as f32 },
            xf: Transform {
                translate: [center[0] as f32, center[1] as f32, center[2] as f32],
                ..Transform::IDENTITY
            },
        }
    }

    /// A handful of deterministic near-edge seeds (no RNG in the kernel).
    /// Each is a true edge point nudged off in a fixed direction.
    fn nudges() -> [[f64; 3]; 6] {
        [
            [0.30, -0.20, 0.10],
            [-0.25, 0.15, -0.30],
            [0.05, 0.40, 0.05],
            [-0.35, -0.10, 0.20],
            [0.18, 0.18, -0.18],
            [-0.12, -0.28, -0.06],
        ]
    }

    #[test]
    fn sphere_plane_projects_onto_exact_circle() {
        // Unit sphere at origin ∩ plane z = 0.3 → circle radius √(1-0.09).
        let s = sphere([0.0, 0.0, 0.0], 1.0);
        let pl = plane([0.0, 0.0, 1.0], 0.3);
        let expected_r = (1.0 - 0.3 * 0.3_f64).sqrt();
        // Seeds near the +x point of the circle.
        let base = [expected_r, 0.0, 0.3];
        for d in nudges() {
            let p0 = add(base, d);
            let p = project_to_edge(&s, &pl, p0).expect("must converge");
            // On the sphere: |p| == 1 (radius 1.0 is f32-exact → tight).
            assert!((norm(p) - 1.0).abs() < 1e-9, "|p|={} for seed {p0:?}", norm(p));
            // On the plane: z == 0.3 (offset stored as f32 → f32 floor).
            assert!((p[2] - 0.3).abs() < 1e-6, "z={} for seed {p0:?}", p[2]);
            // On the circle: radius in z=0.3 plane.
            let r = (p[0] * p[0] + p[1] * p[1]).sqrt();
            assert!((r - expected_r).abs() < 1e-6, "r={r} expected {expected_r}");
        }
    }

    #[test]
    fn cylinder_plane_projects_onto_cap_circle() {
        // Cylinder radius 2 about Y at origin ∩ plane y = 1.5 → circle r=2 @ y=1.5.
        let c = cylinder_y([0.0, 0.0, 0.0], 2.0);
        let pl = plane([0.0, 1.0, 0.0], 1.5);
        let base = [2.0, 1.5, 0.0];
        for d in nudges() {
            let p0 = add(base, d);
            let p = project_to_edge(&c, &pl, p0).expect("must converge");
            assert!((p[1] - 1.5).abs() < 1e-9, "y={}", p[1]);
            let r = (p[0] * p[0] + p[2] * p[2]).sqrt();
            assert!((r - 2.0).abs() < 1e-9, "r={r} expected 2");
        }
    }

    #[test]
    fn sphere_sphere_projects_onto_radical_circle() {
        // Two unit spheres, centres at ±(0.6,0,0). Seam: plane x=0, circle
        // radius √(1-0.36) in that plane.
        let a = sphere([-0.6, 0.0, 0.0], 1.0);
        let b = sphere([0.6, 0.0, 0.0], 1.0);
        let expected_r = (1.0 - 0.6 * 0.6_f64).sqrt();
        let base = [0.0, expected_r, 0.0];
        for d in nudges() {
            let p0 = add(base, d);
            let p = project_to_edge(&a, &b, p0).expect("must converge");
            // Radical plane x = 0 (symmetric ±0.6 f32 centres → exact).
            assert!(p[0].abs() < 1e-9, "x={}", p[0]);
            // Circle radius vs ideal (centre 0.6 stored as f32 → f32 floor).
            let r = (p[1] * p[1] + p[2] * p[2]).sqrt();
            assert!((r - expected_r).abs() < 1e-6, "r={r} expected {expected_r}");
        }
    }

    #[test]
    fn sphere_cylinder_projects_onto_both_surfaces() {
        // Unit sphere at origin ∩ cylinder radius 0.5 about Y. The seam is a
        // pair of circles at y = ±√(1-0.25); seeds near the upper one. No
        // closed form is asserted — only that the projected point lies on BOTH
        // surfaces (the general guarantee).
        let s = sphere([0.0, 0.0, 0.0], 1.0);
        let c = cylinder_y([0.0, 0.0, 0.0], 0.5);
        let y = (1.0 - 0.25_f64).sqrt();
        let base = [0.5, y, 0.0];
        for d in nudges() {
            let p0 = add(base, [d[0] * 0.3, d[1] * 0.3, d[2] * 0.3]);
            let p = project_to_edge(&s, &c, p0).expect("must converge");
            // On the sphere.
            assert!((norm(p) - 1.0).abs() < 1e-8, "|p|={}", norm(p));
            // On the cylinder.
            let r = (p[0] * p[0] + p[2] * p[2]).sqrt();
            assert!((r - 0.5).abs() < 1e-8, "r={r} expected 0.5");
        }
    }

    #[test]
    fn parallel_planes_rejected_by_rank_guard() {
        // Two parallel planes never meet → tangential, rank-deficient.
        let p1 = plane([0.0, 0.0, 1.0], 0.3);
        let p2 = plane([0.0, 0.0, 1.0], 0.7);
        assert!(project_to_edge(&p1, &p2, [0.1, 0.2, 0.5]).is_none());
    }

    #[test]
    fn edge_tangent_is_perpendicular_to_both_normals() {
        // On the sphere∩plane circle, the tangent must be ⟂ to z (plane normal)
        // and ⟂ to the sphere radius.
        let s = sphere([0.0, 0.0, 0.0], 1.0);
        let pl = plane([0.0, 0.0, 1.0], 0.3);
        let r = (1.0 - 0.09_f64).sqrt();
        let p = project_to_edge(&s, &pl, [r + 0.1, 0.05, 0.32]).unwrap();
        let t = edge_tangent(&s, &pl, p).unwrap();
        assert!(t[2].abs() < 1e-9, "tangent must be in-plane, tz={}", t[2]);
        // ⟂ to sphere normal (= p, since centred at origin).
        assert!(dot(t, p).abs() < 1e-9, "tangent must be ⟂ radius");
        assert!((norm(t) - 1.0).abs() < 1e-12, "unit tangent");
    }

    #[test]
    fn leaf_field_projects_like_analytic() {
        // The general Leaf path (dual SDF) must land on the same circle as the
        // analytic half-space for an analytic primitive — proves the projection
        // is field-agnostic, not secretly relying on the closed form.
        use crate::bvh::BoundedCsg;
        use crate::csg::{CsgNode, DecodedFeature, PrimitiveModifiers};
        use fielddraft_protocol::{BoolOp, FeatureId, Primitive};

        let sphere_feat = DecodedFeature::from_parts(
            FeatureId::from_u32(1),
            "S".into(),
            BoolOp::Union,
            Primitive::Sphere { center: [0.0, 0.0, 0.0], radius: 1.0 },
            Transform::IDENTITY,
            PrimitiveModifiers::NONE,
            vec![],
        );
        let csg = CsgNode::from_decoded(&[sphere_feat]);
        let leaf = BoundedCsg::from_csg(&csg);
        let s = Field::Leaf { node: &leaf };
        let pl = plane([0.0, 0.0, 1.0], 0.3);
        let expected_r = (1.0 - 0.09_f64).sqrt();
        let base = [expected_r, 0.0, 0.3];
        for d in nudges() {
            let p0 = add(base, [d[0] * 0.3, d[1] * 0.3, d[2] * 0.3]);
            let p = project_to_edge(&s, &pl, p0).expect("leaf path must converge");
            assert!((norm(p) - 1.0).abs() < 1e-7, "|p|={}", norm(p));
            assert!((p[2] - 0.3).abs() < 1e-7, "z={}", p[2]);
        }
    }
}
