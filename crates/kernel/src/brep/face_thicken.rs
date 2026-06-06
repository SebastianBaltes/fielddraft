//! Derive a thin-shell solid from a **curved** analytic B-Rep face's surface.
//!
//! This is the kernel half of "thicken a face" — the curved counterpart to
//! [`crate::brep::face_profile`] (which extrudes a *planar* face). Onshape's
//! Thicken takes a curved face's analytic surface and offsets it along its
//! normal by a wall thickness to make a thin solid that follows the curvature.
//!
//! ## Reduction (same trick as planar extrude)
//!
//! For the analytic surfaces whose normal offset is a pure parameter shift, a
//! thickened wall is just the **difference of two coaxial offset solid
//! primitives**, so it reuses the whole solid pipeline (SDF, weave, booleans)
//! for free — exactly like a planar face extrude reused [`crate::Primitive::Extrude`]:
//!
//! - `Sphere(r)`      → `Sphere(r+out) − Sphere(r−in)`         (radial normal)
//! - `Cylinder(r)`    → `Cyl(r+out, h) − Cyl(r−in, h)`         (radial normal;
//!   the axial extent `h` is recovered from the face's rim loops)
//! - `Torus(R, r)`    → `Torus(R, r+out) − Torus(R, r−in)`     (tube-radial normal)
//!
//! Closed surfaces (sphere, torus) need no boundary at all. A cylinder wall
//! needs its axial span, read off the rim loops in surface-local `y`, plus a
//! full-revolution check (a cut / partial cylinder would be overshot by a
//! difference of *full* cylinders, so it is deferred).
//!
//! ## Coordinate spaces
//!
//! The returned [`ThickenSolid::outer`]/`inner` primitives live in the face's
//! **surface-local** frame (the frame in which [`HalfSpace`] is centred), and
//! [`ThickenSolid::world`] maps that frame to world (`placement ∘
//! face.transform`). The caller wraps the `Difference` of the two primitives in
//! `world` as the feature placement — so a single-face thicken keeps a
//! separable placement (DL-56 move-cache).
//!
//! ## Out of scope (first throw) — each returns a descriptive `Err`
//!
//! Cone (apex convention, DL-76), partial/cut cylinders, planar faces (use
//! Extrude with `symmetric`), and tessellated/TPMS faces.

use fielddraft_protocol::{HalfSpace, Primitive, Transform};

use crate::brep::{BRep, FaceId, Loop, SurfaceGeometry};

/// The two coaxial offset solids whose difference is the thickened wall, plus
/// the world placement that maps their surface-local frame to world.
#[derive(Debug, Clone)]
pub struct ThickenSolid {
    /// Outer (grown along +normal) solid in surface-local coordinates.
    pub outer: Primitive,
    /// Inner (shrunk along −normal) solid — subtracted to leave the wall.
    pub inner: Primitive,
    /// Surface-local → world placement (`placement ∘ face.transform`).
    pub world: Transform,
}

/// Build the offset-solid pair for a curved analytic face. `outer_off` grows
/// the surface along +normal (outside the source solid), `inner_off` shrinks it
/// along −normal; both must be `≥ 0`. Returns a descriptive `Err` for planar,
/// cone, tessellated, partial-revolution or degenerate faces.
pub fn thicken_face(
    brep: &BRep,
    face_id: FaceId,
    placement: &Transform,
    outer_off: f32,
    inner_off: f32,
) -> Result<ThickenSolid, String> {
    let SurfaceGeometry::Analytic { surface, transform } = &brep.face(face_id).surface else {
        return Err("face is tessellated (TPMS / non-analytic) — cannot thicken".to_string());
    };
    // Surface-local → world. The face transform places the centred HalfSpace
    // into the body, the body placement into the world.
    let world = crate::features::compose_placement(placement, transform);

    let (outer, inner) = match surface {
        HalfSpace::Plane { .. } => {
            return Err(
                "planar face — use Extrude (Symmetric) to thicken a flat face".to_string()
            );
        }
        HalfSpace::Sphere { radius } => {
            let r = *radius;
            guard_inner(r, inner_off)?;
            (
                Primitive::Sphere { center: [0.0, 0.0, 0.0], radius: r + outer_off },
                Primitive::Sphere { center: [0.0, 0.0, 0.0], radius: r - inner_off },
            )
        }
        HalfSpace::Torus { major_radius, minor_radius } => {
            let r = *minor_radius;
            guard_inner(r, inner_off)?;
            (
                Primitive::Torus {
                    center: [0.0, 0.0, 0.0],
                    major_radius: *major_radius,
                    minor_radius: r + outer_off,
                },
                Primitive::Torus {
                    center: [0.0, 0.0, 0.0],
                    major_radius: *major_radius,
                    minor_radius: r - inner_off,
                },
            )
        }
        HalfSpace::InfiniteCylinder { radius } => {
            let r = *radius;
            guard_inner(r, inner_off)?;
            // The HalfSpace cylinder is infinite along local Y; the face's rim
            // loops give the real axial slab to bound the tube.
            let (cy, hh) = cylinder_axial_extent(brep, face_id, transform)?;
            (
                Primitive::Cylinder {
                    center: [0.0, cy, 0.0],
                    radius: r + outer_off,
                    half_height: hh,
                },
                Primitive::Cylinder {
                    center: [0.0, cy, 0.0],
                    radius: r - inner_off,
                    half_height: hh,
                },
            )
        }
        HalfSpace::Cone { .. } => {
            return Err(
                "conical face thicken is deferred (apex convention, DL-76)".to_string()
            );
        }
        HalfSpace::Gyroid { .. } => {
            return Err("gyroid / TPMS face cannot be thickened analytically".to_string());
        }
    };
    Ok(ThickenSolid { outer, inner, world })
}

/// Reject an inward offset that would consume the surface (radius ≤ 0): the
/// inner wall would invert / self-intersect.
fn guard_inner(radius: f32, inner_off: f32) -> Result<(), String> {
    if radius - inner_off <= 1e-4 {
        return Err(format!(
            "inward thickness {inner_off} ≥ surface radius {radius} — wall would self-intersect"
        ));
    }
    Ok(())
}

/// Recover a cylindrical wall's axial centre + half-height (surface-local `y`)
/// from its rim loops, and verify the wall is a **full** revolution. A cut /
/// partial cylinder has axial boundary edges that leave an angular gap, which a
/// difference of *full* cylinders would overshoot — so that case is deferred.
fn cylinder_axial_extent(
    brep: &BRep,
    face_id: FaceId,
    surf: &Transform,
) -> Result<(f32, f32), String> {
    // body-local → surface-local (the frame the HalfSpace is centred in).
    let inv = crate::inverse(surf);
    let face = brep.face(face_id);

    let mut lo = f32::INFINITY;
    let mut hi = f32::NEG_INFINITY;
    let mut thetas: Vec<f32> = Vec::new();
    let mut sample_loop = |lp: &Loop| {
        for &(edge_id, _reversed) in &lp.edges {
            for p in crate::brep::edge_sampling::sample_edge(&brep.edge(edge_id).geometry) {
                let pl = place_point(&inv, [p[0] as f32, p[1] as f32, p[2] as f32]);
                lo = lo.min(pl[1]);
                hi = hi.max(pl[1]);
                // Azimuth around the cylinder axis (local Y); the seam line all
                // shares one θ but the rims must sweep the full circle.
                thetas.push(pl[2].atan2(pl[0]));
            }
        }
    };
    sample_loop(&face.outer);
    for hole in &face.inner {
        sample_loop(hole);
    }

    if !lo.is_finite() || !hi.is_finite() {
        return Err("cylindrical face has no boundary rim to size the wall".to_string());
    }
    let hh = (hi - lo) * 0.5;
    if hh <= 1e-4 {
        return Err("cylindrical face is degenerate (zero axial height)".to_string());
    }
    require_full_revolution(&mut thetas)?;
    Ok(((lo + hi) * 0.5, hh))
}

/// The rim azimuths must wrap the whole circle with no large gap; otherwise the
/// face is a partial (cut) cylinder a full-cylinder difference would overshoot.
fn require_full_revolution(thetas: &mut [f32]) -> Result<(), String> {
    if thetas.len() < 3 {
        return Err("cylindrical face boundary is too sparse to verify revolution".to_string());
    }
    thetas.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mut max_gap = 0.0_f32;
    for w in thetas.windows(2) {
        max_gap = max_gap.max(w[1] - w[0]);
    }
    // Wrap-around gap between the largest and smallest angle.
    let span = thetas[thetas.len() - 1] - thetas[0];
    max_gap = max_gap.max(std::f32::consts::TAU - span);
    // ~34°: comfortably above the ~22.5° spacing of a 16-sample rim, well below
    // the ≥90° wedge a real cut would remove.
    if max_gap > 0.6 {
        return Err(
            "partial (cut) cylindrical face — only full-revolution walls thicken for now"
                .to_string(),
        );
    }
    Ok(())
}

/// Apply a [`Transform`] (scale → rotate → translate) to a point. Mirrors the
/// mapping in [`crate::brep::face_profile`] / `face_plane_frame`.
fn place_point(t: &Transform, p: [f32; 3]) -> [f32; 3] {
    let scaled = [p[0] * t.scale[0], p[1] * t.scale[1], p[2] * t.scale[2]];
    let r = crate::quat_rotate(t.rotation, scaled);
    [r[0] + t.translate[0], r[1] + t.translate[1], r[2] + t.translate[2]]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_revolution_accepts_a_dense_circle() {
        // 24 samples around a full circle → max gap ≈ 15° < threshold.
        let mut thetas: Vec<f32> = (0..24)
            .map(|i| (i as f32 / 24.0) * std::f32::consts::TAU - std::f32::consts::PI)
            .collect();
        assert!(require_full_revolution(&mut thetas).is_ok());
    }

    #[test]
    fn full_revolution_rejects_a_quarter_wedge_gap() {
        // A circle missing a 90° wedge: samples only over [0, 270°).
        let mut thetas: Vec<f32> = (0..=18)
            .map(|i| (i as f32 / 24.0) * std::f32::consts::TAU)
            .collect();
        assert!(require_full_revolution(&mut thetas).is_err());
    }

    #[test]
    fn place_point_round_trips_through_inverse() {
        let t = Transform {
            translate: [3.0, -2.0, 1.0],
            rotation: crate::features::euler_xyz_to_quat(10.0, 25.0, -40.0),
            scale: [1.0, 1.0, 1.0],
        };
        let p = [1.5, 0.5, -0.7];
        let there = place_point(&t, p);
        let back = place_point(&crate::inverse(&t), there);
        for k in 0..3 {
            assert!((back[k] - p[k]).abs() < 1e-4, "axis {k}: {} vs {}", back[k], p[k]);
        }
    }
}
