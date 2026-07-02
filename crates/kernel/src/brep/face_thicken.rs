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
//! - `Cone(α)`        → `Frustum(r±Δ) − Frustum(r∓Δ)`          (slant normal;
//!   a *normal* offset `t` is a *radial* shift `Δ = t/cos α` of both rim radii,
//!   leaving the half-angle unchanged — a parallel cone. The frustum's two rim
//!   radii + axial span are recovered from the rim loops; DL-76's apex-correct
//!   cone weave makes those rims exist and the offset frustums weave watertight.)
//!
//! Closed surfaces (sphere, torus) need no boundary at all. A cylinder or cone
//! wall needs its axial span, read off the rim loops in surface-local `y`, plus a
//! full-revolution check (a cut / partial wall would be overshot by a difference
//! of *full* solids, so it is deferred).
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
//! ## Scaled placements
//!
//! The offsets are given in world millimetres but the offset solids are built in
//! the surface-local frame, so a uniform body scale `s` is divided out
//! (`δ_local = t / s`) — a thicken reads the same wall thickness whatever the
//! body's scale. A *non-uniform* scale is rejected: the offset of these surfaces
//! under anisotropic scale is not an analytic primitive.
//!
//! ## Up-to-face targets
//!
//! Instead of a fixed thickness the wall can grow up to a coaxial **target**
//! face: [`offset_to_target`] measures the signed world-mm normal gap between a
//! source surface and a coaxial target of the *same* analytic kind (concentric
//! sphere, coaxial cylinder, coaxial torus) and turns it into the `(outer, inner)`
//! offsets [`thicken_face`] already consumes — so the whole reduction above is
//! reused verbatim. The gap's sign picks the direction (target outside → outward
//! wall, inside → inward). A non-coaxial, type-mismatched or cone target is
//! rejected (its normal gap is not a constant offset, so the wall would not be a
//! difference of two coaxial primitives).
//!
//! ## Out of scope (first throw) — each returns a descriptive `Err`
//!
//! Partial/cut cylinders & cones, planar faces (use Extrude with `symmetric`),
//! tessellated/TPMS faces, and cone up-to-face targets.

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
/// along −normal; both are world millimetres and `≥ 0`. Returns a descriptive
/// `Err` for planar, tessellated, partial-revolution, non-uniformly-scaled or
/// degenerate faces.
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
    // into the body, the body placement into the world. The cone arm folds its
    // axial centre into `world` (a `Primitive::Cone` is always origin-centred),
    // so it is mutable.
    let mut world = crate::features::compose_placement(placement, transform);

    // Eikonal-renormalise the offsets to world millimetres. The offset solids
    // are built in the surface-local frame and then placed by `world`, which may
    // carry a body scale `s`: a local radius delta `δ` shows up as `s·δ` in the
    // model. So a *world* wall thickness `t` is a *local* shift `t/s`. Without
    // this, a 2×-scaled body would render a 2 mm thicken as a 4 mm wall.
    //
    // A *non-uniform* scale is rejected: the normal offset of a sphere (or any
    // of these surfaces) under anisotropic scale is an offset ellipsoid, not an
    // analytic primitive — the whole parameter-shift reduction breaks. Honest
    // error beats silently-wrong geometry.
    let (outer_off, inner_off) = {
        let s = uniform_scale(&world)?;
        (outer_off / s, inner_off / s)
    };

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
        HalfSpace::Cone { half_angle } => {
            // The face transform's origin is the cone apex (DL-76); the rim loops
            // give the axial slab and the two rim radii. A *normal* offset `t` is
            // a *radial* shift `Δ = t / cos α` of both rims (parallel cone, same
            // half-angle).
            let (cy, hh, r_bottom, r_top) = cone_frustum_extent(brep, face_id, transform)?;
            let cos_a = half_angle.cos().abs().max(1e-4);
            let d_out = outer_off / cos_a;
            let d_in = inner_off / cos_a;
            // The narrower rim limits how far the wall can move inward.
            guard_inner(r_bottom.min(r_top), d_in)?;
            // A `Primitive::Cone` is origin-centred (no `center` field), so fold
            // the frustum's axial centre into the placement: translate by `cy`
            // along the surface-local axis, then map to world.
            world = crate::features::compose_placement(
                &world,
                &Transform { translate: [0.0, cy, 0.0], ..Transform::IDENTITY },
            );
            (
                Primitive::Cone {
                    r_bottom: r_bottom + d_out,
                    r_top: r_top + d_out,
                    half_height: hh,
                },
                Primitive::Cone {
                    r_bottom: r_bottom - d_in,
                    r_top: r_top - d_in,
                    half_height: hh,
                },
            )
        }
        HalfSpace::Gyroid { .. } => {
            return Err("gyroid / TPMS face cannot be thickened analytically".to_string());
        }
        // The thicken result must be a Primitive pair; a revolved profile
        // group has no primitive counterpart (yet).
        HalfSpace::RevolvedProfile { .. } => {
            return Err("revolved profile face cannot be thickened analytically".to_string());
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

/// Verify a surface-local → world placement carries a **uniform**, non-degenerate
/// scale and return its signed magnitude `s` (local length × `s` = world length).
/// The analytic normal-offset reduction only holds under uniform scale — an
/// anisotropic scale turns these surfaces' offsets into non-primitive (ellipsoid)
/// shapes — so a non-uniform scale is an honest `Err` rather than silently-wrong
/// geometry.
fn uniform_scale(world: &Transform) -> Result<f32, String> {
    let s = world.scale;
    let uniform = (s[0] - s[1]).abs() < 1e-4 * s[0].abs().max(1.0)
        && (s[1] - s[2]).abs() < 1e-4 * s[1].abs().max(1.0);
    if !uniform {
        return Err(format!(
            "face has a non-uniform scale ({:.3}, {:.3}, {:.3}) — its normal offset is not an \
             analytic primitive; apply a uniform scale or thicken before scaling",
            s[0], s[1], s[2]
        ));
    }
    if s[0].abs() < 1e-6 {
        return Err("face placement scale is degenerate (zero) — cannot thicken".to_string());
    }
    Ok(s[0])
}

/// A source/target analytic surface lifted to world space for the up-to-face
/// mode: its [`HalfSpace`] (with surface-local params), its surface-local → world
/// placement, and the uniform world scale magnitude.
struct WorldSurface {
    kind: HalfSpace,
    world: Transform,
    /// World scale magnitude (`local_radius × s_abs = world_radius`).
    s_abs: f32,
}

/// Lift a face's analytic surface to world space (see [`WorldSurface`]). Errors
/// for tessellated faces and non-uniform / degenerate scales.
fn world_surface(brep: &BRep, face_id: FaceId, placement: &Transform) -> Result<WorldSurface, String> {
    let SurfaceGeometry::Analytic { surface, transform } = &brep.face(face_id).surface else {
        return Err("face is tessellated (TPMS / non-analytic) — cannot use as a thicken target".to_string());
    };
    let world = crate::features::compose_placement(placement, transform);
    let s_abs = uniform_scale(&world)?.abs();
    Ok(WorldSurface { kind: surface.clone(), world, s_abs })
}

/// Compute the `(outer_off, inner_off)` world-millimetre offsets that grow a
/// `source` face's surface up to a coaxial `target` face — the up-to-face thicken
/// mode. Both surfaces must be the **same** analytic kind and coaxial; the signed
/// normal gap decides the direction (target outside → outward wall, inside →
/// inward). First throw: sphere, cylinder, torus. A cone target, a type mismatch,
/// a non-coaxial target or a coincident target each returns a descriptive `Err`.
///
/// The returned offsets are world millimetres, so the caller feeds them straight
/// into [`thicken_face`] (which renormalises them by the source's own scale) — the
/// outer/inner solid then reaches exactly the target surface.
pub fn offset_to_target(
    src_brep: &BRep,
    src_face: FaceId,
    src_placement: &Transform,
    tgt_brep: &BRep,
    tgt_face: FaceId,
    tgt_placement: &Transform,
) -> Result<(f32, f32), String> {
    let src = world_surface(src_brep, src_face, src_placement)?;
    let tgt = world_surface(tgt_brep, tgt_face, tgt_placement)?;
    let gap = coaxial_gap(&src, &tgt)?;
    if gap.abs() < 1e-4 {
        return Err(
            "target face is coincident with the source surface — there is no wall to fill".to_string(),
        );
    }
    if gap > 0.0 {
        Ok((gap, 0.0)) // target outside the source → grow outward
    } else {
        Ok((0.0, -gap)) // target inside → grow inward
    }
}

/// The signed world-mm normal gap `target − source` between two coaxial analytic
/// surfaces of the same kind. Errors when the kinds differ, the target is a cone
/// (deferred), or the two surfaces are not coaxial.
fn coaxial_gap(src: &WorldSurface, tgt: &WorldSurface) -> Result<f32, String> {
    match (&src.kind, &tgt.kind) {
        (HalfSpace::Sphere { radius: rs }, HalfSpace::Sphere { radius: rt }) => {
            let rs_w = rs * src.s_abs;
            let rt_w = rt * tgt.s_abs;
            let tol = 1e-3 * rs_w.max(rt_w).max(1.0);
            if dist(src.world.translate, tgt.world.translate) > tol {
                return Err(
                    "source and target spheres are not concentric — up-to-face needs a coaxial target"
                        .to_string(),
                );
            }
            Ok(rt_w - rs_w)
        }
        (HalfSpace::InfiniteCylinder { radius: rs }, HalfSpace::InfiniteCylinder { radius: rt }) => {
            let rs_w = rs * src.s_abs;
            let rt_w = rt * tgt.s_abs;
            require_coaxial_axis(src, tgt, 1e-3 * rs_w.max(rt_w).max(1.0))?;
            Ok(rt_w - rs_w)
        }
        (
            HalfSpace::Torus { major_radius: rmaj_s, minor_radius: rs },
            HalfSpace::Torus { major_radius: rmaj_t, minor_radius: rt },
        ) => {
            let rmaj_s_w = rmaj_s * src.s_abs;
            let rmaj_t_w = rmaj_t * tgt.s_abs;
            let tol = 1e-3 * rmaj_s_w.max(rmaj_t_w).max(1.0);
            require_coaxial_axis(src, tgt, tol)?;
            if dist(src.world.translate, tgt.world.translate) > tol
                || (rmaj_s_w - rmaj_t_w).abs() > tol
            {
                return Err(
                    "source and target tori are not coaxial with an equal major radius".to_string(),
                );
            }
            Ok(rt * tgt.s_abs - rs * src.s_abs)
        }
        (HalfSpace::Cone { .. }, _) | (_, HalfSpace::Cone { .. }) => Err(
            "cone up-to-face targets are deferred — give the wall an explicit thickness instead"
                .to_string(),
        ),
        (HalfSpace::Plane { .. }, _) | (_, HalfSpace::Plane { .. }) => Err(
            "planar faces are not a curved thicken source/target — use Extrude".to_string(),
        ),
        _ => Err(
            "source and target faces must be the same coaxial analytic kind \
             (sphere/cylinder/torus) for up-to-face"
                .to_string(),
        ),
    }
}

/// Require two surfaces' local-Y axes to be the same world line (parallel and
/// colinear within `tol`). Used for the cylinder/torus up-to-face targets.
fn require_coaxial_axis(src: &WorldSurface, tgt: &WorldSurface, tol: f32) -> Result<(), String> {
    let ds = crate::quat_rotate(src.world.rotation, [0.0, 1.0, 0.0]);
    let dt = crate::quat_rotate(tgt.world.rotation, [0.0, 1.0, 0.0]);
    let dot = ds[0] * dt[0] + ds[1] * dt[1] + ds[2] * dt[2];
    if dot.abs() < 1.0 - 1e-3 {
        return Err(
            "source and target axes are not parallel — up-to-face needs a coaxial target".to_string(),
        );
    }
    // Perpendicular distance between the two axis lines: the component of the
    // centre-to-centre vector orthogonal to the (unit) axis direction.
    let ps = src.world.translate;
    let pt = tgt.world.translate;
    let w = [pt[0] - ps[0], pt[1] - ps[1], pt[2] - ps[2]];
    let wd = w[0] * ds[0] + w[1] * ds[1] + w[2] * ds[2];
    let perp = [w[0] - wd * ds[0], w[1] - wd * ds[1], w[2] - wd * ds[2]];
    let off = (perp[0] * perp[0] + perp[1] * perp[1] + perp[2] * perp[2]).sqrt();
    if off > tol {
        return Err(
            "source and target axes are offset — up-to-face needs a coaxial target".to_string(),
        );
    }
    Ok(())
}

/// Euclidean distance between two points.
fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
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

/// Recover a conical wall's frustum parameters — axial centre + half-height and
/// the two rim radii (`r_bottom` at the lower `y`, `r_top` at the upper) — in the
/// **apex-local** surface frame (the cone's transform origin is its apex, DL-76),
/// and verify the wall is a **full** revolution and does not straddle the apex.
/// Returns `(center_y, half_height, r_bottom, r_top)`.
fn cone_frustum_extent(
    brep: &BRep,
    face_id: FaceId,
    surf: &Transform,
) -> Result<(f32, f32, f32, f32), String> {
    // body-local → surface-local (apex-centred frame the HalfSpace is in).
    let inv = crate::inverse(surf);
    let face = brep.face(face_id);

    let mut lo = f32::INFINITY;
    let mut hi = f32::NEG_INFINITY;
    let mut r_at_lo = 0.0_f32;
    let mut r_at_hi = 0.0_f32;
    let mut thetas: Vec<f32> = Vec::new();
    let mut sample_loop = |lp: &Loop| {
        for &(edge_id, _reversed) in &lp.edges {
            for p in crate::brep::edge_sampling::sample_edge(&brep.edge(edge_id).geometry) {
                let pl = place_point(&inv, [p[0] as f32, p[1] as f32, p[2] as f32]);
                let r = (pl[0] * pl[0] + pl[2] * pl[2]).sqrt();
                if pl[1] < lo {
                    lo = pl[1];
                    r_at_lo = r;
                }
                if pl[1] > hi {
                    hi = pl[1];
                    r_at_hi = r;
                }
                thetas.push(pl[2].atan2(pl[0]));
            }
        }
    };
    sample_loop(&face.outer);
    for hole in &face.inner {
        sample_loop(hole);
    }

    if !lo.is_finite() || !hi.is_finite() {
        return Err("conical face has no boundary rim to size the wall".to_string());
    }
    // Both rims must sit on the same nappe — a wall that spanned the apex (apex at
    // surface-local y = 0) would be overshot by a difference of full cones.
    if lo < -1e-4 && hi > 1e-4 {
        return Err("conical face spans the apex — cannot thicken as a single frustum".to_string());
    }
    let hh = (hi - lo) * 0.5;
    if hh <= 1e-4 {
        return Err("conical face is degenerate (zero axial height)".to_string());
    }
    require_full_revolution(&mut thetas)?;
    Ok(((lo + hi) * 0.5, hh, r_at_lo, r_at_hi))
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
            rotation: crate::transform::euler_xyz_to_quat(10.0, 25.0, -40.0),
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
