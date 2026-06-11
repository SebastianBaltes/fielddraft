//! Pointwise geometry queries on analytic surfaces: normals, uv
//! parameters, tangent projection and signed angles.


use crate::brep::{
    BRep, EdgeGeometry, EdgeId,
    SurfaceGeometry, VertexId,
};
use fielddraft_protocol::HalfSpace;

// ─── small vector helpers (f64): shared, see brep::vec3 ─────────────────────
use crate::brep::vec3::{normalize as unit, sub};
use crate::brep::vec3::norm;

/// The world-space outward surface normal of `surface` at world point `p`, used
/// to build the tangent plane for turning-angle ring ordering. Returns `None`
/// for surfaces without a usable point normal (tessellated / gyroid) — the
/// caller then falls back to the topology-only greedy walk.
///
/// - `Plane`: the (rotated) plane normal — constant over the face.
/// - `InfiniteCylinder` / `Cone`: the radial direction from the local axis
///   (local +Y) to `p`, rotated to world. (For a cone this is the radial, not
///   the true slant normal, but it is a stable, point-dependent tangent-plane
///   reference, which is all the ring ordering needs.)
/// - `Sphere`: the unit vector from the centre to `p`.
/// - `Torus`: the direction from the nearest point on the tube centre-circle
///   (radius = major) to `p`.
pub(super) fn surface_normal_at(surface: &SurfaceGeometry, p: [f64; 3]) -> Option<[f64; 3]> {
    let SurfaceGeometry::Analytic { surface, transform } = surface else {
        return None;
    };
    let rot = transform.rotation;
    let to_local = |p: [f64; 3]| -> [f64; 3] {
        let w = [p[0] as f32, p[1] as f32, p[2] as f32];
        let l = crate::world_to_local(transform, w);
        [l[0] as f64, l[1] as f64, l[2] as f64]
    };
    let rot_to_world = |d: [f64; 3]| -> [f64; 3] {
        let r = crate::quat_rotate(rot, [d[0] as f32, d[1] as f32, d[2] as f32]);
        unit([r[0] as f64, r[1] as f64, r[2] as f64])
    };
    match surface {
        HalfSpace::Plane { normal, .. } => {
            Some(rot_to_world([normal[0] as f64, normal[1] as f64, normal[2] as f64]))
        }
        HalfSpace::InfiniteCylinder { .. } | HalfSpace::Cone { .. } => {
            let l = to_local(p);
            let radial = [l[0], 0.0, l[2]]; // strip the local +Y axis component
            if norm(radial) < 1e-9 {
                return None;
            }
            Some(rot_to_world(radial))
        }
        HalfSpace::Sphere { .. } => {
            let l = to_local(p);
            if norm(l) < 1e-9 {
                return None;
            }
            Some(rot_to_world(l))
        }
        HalfSpace::Torus { major_radius, .. } => {
            let l = to_local(p);
            let r = norm([l[0], 0.0, l[2]]);
            if r < 1e-9 {
                return None;
            }
            // Nearest point on the centre-circle (radius = major, in the local
            // x-z plane), then the outward tube direction toward `p`.
            let scale = *major_radius as f64 / r;
            let center = [l[0] * scale, 0.0, l[2] * scale];
            let dir = sub(l, center);
            if norm(dir) < 1e-9 {
                return None;
            }
            Some(rot_to_world(dir))
        }
        HalfSpace::Gyroid { .. } => None,
    }
}

/// Inverse parameterization of an analytic surface: map a world point lying on
/// the surface to its `(u, v)` parameters. Used to classify outer vs. inner
/// loops on curved faces by parametric signed area — a containment test that,
/// unlike world-space chord length, is invariant to a tapered surface's varying
/// radius (a small-radius outer boundary can have shorter chords than a
/// large-radius hole, which the chord proxy would misrank). Returns `None` for
/// surfaces without a usable parameterization (plane / gyroid / tessellated) or
/// at a parametric pole where `(u, v)` is undefined.
///
/// Local-frame conventions match [`surface_normal_at`] (axis = local +Y):
/// - `InfiniteCylinder` / `Cone`: `u = atan2(z, x)` around the axis, `v = y`
///   along it.
/// - `Sphere`: `u = atan2(z, x)` longitude, `v = atan2(y, r_xz)` latitude.
/// - `Torus`: `u = atan2(z, x)` major angle, `v = atan2(y, r_xz − major)` minor.
pub(super) fn surface_uv(surface: &SurfaceGeometry, p: [f64; 3]) -> Option<(f64, f64)> {
    let SurfaceGeometry::Analytic { surface, transform } = surface else {
        return None;
    };
    let l = crate::world_to_local(transform, [p[0] as f32, p[1] as f32, p[2] as f32]);
    let l = [l[0] as f64, l[1] as f64, l[2] as f64];
    let r_xz = norm([l[0], 0.0, l[2]]);
    match surface {
        HalfSpace::InfiniteCylinder { .. } | HalfSpace::Cone { .. } => {
            if r_xz < 1e-9 {
                return None;
            }
            Some((l[2].atan2(l[0]), l[1]))
        }
        HalfSpace::Sphere { .. } => {
            if r_xz < 1e-9 && l[1].abs() < 1e-9 {
                return None;
            }
            Some((l[2].atan2(l[0]), l[1].atan2(r_xz)))
        }
        HalfSpace::Torus { major_radius, .. } => {
            if r_xz < 1e-9 {
                return None;
            }
            Some((l[2].atan2(l[0]), l[1].atan2(r_xz - *major_radius as f64)))
        }
        HalfSpace::Plane { .. } | HalfSpace::Gyroid { .. } => None,
    }
}

/// Project a 3D direction into the tangent plane defined by unit `normal`,
/// dropping its normal component. (Not re-normalised; callers compare angles.)
pub(super) fn project_tangent(d: [f64; 3], normal: [f64; 3]) -> [f64; 3] {
    let dn = d[0] * normal[0] + d[1] * normal[1] + d[2] * normal[2];
    [
        d[0] - dn * normal[0],
        d[1] - dn * normal[1],
        d[2] - dn * normal[2],
    ]
}

/// Signed CCW angle (about `normal`) needed to rotate tangent vector `from`
/// onto `to`, in `(-π, π]`. Positive = counter-clockwise seen from the side
/// `normal` points toward.
pub(super) fn signed_angle(from: [f64; 3], to: [f64; 3], normal: [f64; 3]) -> f64 {
    let cross = [
        from[1] * to[2] - from[2] * to[1],
        from[2] * to[0] - from[0] * to[2],
        from[0] * to[1] - from[1] * to[0],
    ];
    let s = cross[0] * normal[0] + cross[1] * normal[1] + cross[2] * normal[2];
    let c = from[0] * to[0] + from[1] * to[1] + from[2] * to[2];
    s.atan2(c)
}

/// Outgoing world-space tangent of edge `eid` *leaving* vertex `at` (i.e. when
/// the edge is oriented to start at `at`). Uses the adjacent curve sample so a
/// curved edge contributes its true departure direction; falls back to the
/// straight chord between endpoints.
pub(super) fn outgoing_tangent(brep: &BRep, eid: EdgeId, at: VertexId, reversed: bool) -> Option<[f64; 3]> {
    let edge = brep.edge(eid);
    let pts: Vec<[f64; 3]> = match &edge.geometry {
        EdgeGeometry::Nurbs(c) => c.control_points.clone(),
        EdgeGeometry::Polyline(p) => p.clone(),
        EdgeGeometry::Hermite(s) => s.iter().map(|s| s.point).collect(),
    };
    let at_pos = brep.vertex(at).position;
    // Direction from the vertex toward the first distinct interior point along
    // the traversal direction.
    let ordered: Vec<[f64; 3]> = if reversed {
        pts.iter().rev().cloned().collect()
    } else {
        pts
    };
    for p in &ordered {
        let d = sub(*p, at_pos);
        if norm(d) > 1e-7 {
            return Some(unit(d));
        }
    }
    // No usable curve samples (e.g. an empty polyline in hand-built test
    // geometry): fall back to the straight chord toward the far endpoint. With
    // `reversed` the edge is walked end → start, so the far vertex is `start`;
    // otherwise it is `end`.
    let far = if reversed { edge.start } else { edge.end }?;
    let d = sub(brep.vertex(far).position, at_pos);
    if norm(d) > 1e-7 {
        Some(unit(d))
    } else {
        None
    }
}
