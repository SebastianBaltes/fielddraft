//! Body-local face geometry export + render-mesh face tagging (Brocken A).
//!
//! Brocken A makes B-Rep **faces** selectable in the viewport. Two pieces live
//! here:
//!
//! 1. **[`face_centroid_normal_local`]** — a representative body-LOCAL point +
//!    outward normal + surface-kind label for one [`Face`], used by the
//!    `bodyBrepFaces` wasm export.
//! 2. **[`correlate_mesh_faces`]** — tags every render-mesh vertex with the
//!    arena index of the B-Rep [`Face`] it lies on (`u32::MAX` sentinel for
//!    unresolved). This is the "mesh-tagging" foundation for face picking: a
//!    hovered triangle resolves to a stable face via the per-vertex tag.
//!
//! The render mesh is **world-space** while the woven B-Rep is **body-local**
//! (DL-56). Correlation brings each mesh vertex back into the body-local frame
//! via `inverse(placement)` before measuring its distance to each candidate
//! analytic face surface.

use std::collections::HashMap;

use fielddraft_protocol::{HalfSpace, Transform};

use crate::brep::{BRep, Face, FaceId, SurfaceGeometry};
use crate::csg::MAX_CONTRIBUTORS;
use crate::dc::CONTRIBUTOR_SENTINEL;
use super::vec3::normalize_or_zero as normalize;

/// Sentinel written into a mesh vertex's face tag when no B-Rep face could be
/// correlated to it (e.g. a smooth blend region with no sharp analytic face,
/// or a vertex on a tessellated surface). Mirrors [`CONTRIBUTOR_SENTINEL`].
pub const FACE_INDEX_SENTINEL: u32 = u32::MAX;

/// Human-readable surface-kind label for a face's surface geometry, matching
/// the `surfaceKind` field of the `bodyBrepFaces` JSON.
pub fn surface_kind(surface: &SurfaceGeometry) -> &'static str {
    match surface {
        SurfaceGeometry::Analytic { surface, .. } => match surface {
            HalfSpace::Plane { .. } => "plane",
            HalfSpace::InfiniteCylinder { .. } => "cylinder",
            HalfSpace::Sphere { .. } => "sphere",
            HalfSpace::Cone { .. } => "cone",
            HalfSpace::Torus { .. } => "torus",
            HalfSpace::Gyroid { .. } => "tessellated",
            HalfSpace::RevolvedProfile { .. } => "revolved_profile",
        },
        SurfaceGeometry::Tessellated { .. } => "tessellated",
    }
}

/// Rotate a vector by a unit quaternion `(x, y, z, w)`.
fn quat_rotate(q: [f32; 4], v: [f32; 3]) -> [f32; 3] {
    let [qx, qy, qz, qw] = q;
    let tx = 2.0 * (qy * v[2] - qz * v[1]);
    let ty = 2.0 * (qz * v[0] - qx * v[2]);
    let tz = 2.0 * (qx * v[1] - qy * v[0]);
    [
        v[0] + qw * tx + qy * tz - qz * ty,
        v[1] + qw * ty + qz * tx - qx * tz,
        v[2] + qw * tz + qx * ty - qy * tx,
    ]
}

/// Body-LOCAL analytic parameters of a curved face's surface, for the hybrid
/// renderer's parametric tessellation (`bodyBrepFaces`' `surfaceParams`). `None`
/// for planar (uses centroid/normal) and tessellated (gyroid/revolved) surfaces.
///
/// The surface transform is body-local — the caller folds the body placement in,
/// exactly like `centroidLocal`/`normalLocal`. The axis of every analytic
/// primitive is local +Y (the primitive convention); the origin is the
/// transform's translation.
pub fn surface_params(surface: &SurfaceGeometry) -> Option<serde_json::Value> {
    let SurfaceGeometry::Analytic { surface, transform } = surface else {
        return None;
    };
    let origin = transform.translate;
    let axis = {
        let r = quat_rotate(transform.rotation, [0.0, 1.0, 0.0]);
        let n = (r[0] * r[0] + r[1] * r[1] + r[2] * r[2]).sqrt();
        if n < 1e-12 { r } else { [r[0] / n, r[1] / n, r[2] / n] }
    };
    match surface {
        HalfSpace::InfiniteCylinder { radius } => Some(serde_json::json!({
            "kind": "cylinder", "origin": origin, "axis": axis, "radius": radius,
        })),
        HalfSpace::Sphere { radius } => Some(serde_json::json!({
            "kind": "sphere", "origin": origin, "radius": radius,
        })),
        HalfSpace::Cone { half_angle } => Some(serde_json::json!({
            "kind": "cone", "origin": origin, "axis": axis, "halfAngle": half_angle,
        })),
        HalfSpace::Torus { major_radius, minor_radius } => Some(serde_json::json!({
            "kind": "torus", "origin": origin, "axis": axis,
            "majorRadius": major_radius, "minorRadius": minor_radius,
        })),
        HalfSpace::Plane { .. }
        | HalfSpace::Gyroid { .. }
        | HalfSpace::RevolvedProfile { .. } => None,
    }
}

/// A representative body-LOCAL centroid + outward unit normal for `face`.
///
/// The centroid is the mean of the face's outer-loop edge sample points
/// (falling back to the tessellated mesh vertices, then the surface origin).
/// The normal is the *outward* surface normal at that centroid: for a plane it
/// is the surface normal mapped through the face transform; for curved analytic
/// surfaces it is the surface-SDF gradient (which always points away from the
/// solid for the half-space convention used by the weaver). Tessellated faces
/// fall back to an area-weighted triangle normal.
pub fn face_centroid_normal_local(brep: &BRep, id: FaceId) -> ([f64; 3], [f64; 3]) {
    let face = brep.face(id);
    let centroid = face_centroid_local(brep, face);
    let normal = face_normal_local(face, centroid);
    (centroid, normal)
}

/// Mean of the outer-loop edge sample points, in the body-local frame.
fn face_centroid_local(brep: &BRep, face: &Face) -> [f64; 3] {
    let mut acc = [0.0f64; 3];
    let mut n = 0usize;
    for &(eid, _) in &face.outer.edges {
        for p in crate::brep::edge_sampling::sample_edge(&brep.edge(eid).geometry) {
            acc[0] += p[0];
            acc[1] += p[1];
            acc[2] += p[2];
            n += 1;
        }
    }
    if n == 0 {
        if let SurfaceGeometry::Tessellated { positions, .. } = &face.surface {
            for p in positions {
                acc[0] += p[0];
                acc[1] += p[1];
                acc[2] += p[2];
                n += 1;
            }
        }
    }
    if n == 0 {
        // No loop / mesh geometry: fall back to the surface's local origin.
        if let SurfaceGeometry::Analytic { transform, .. } = &face.surface {
            return [
                transform.translate[0] as f64,
                transform.translate[1] as f64,
                transform.translate[2] as f64,
            ];
        }
        return [0.0; 3];
    }
    [acc[0] / n as f64, acc[1] / n as f64, acc[2] / n as f64]
}

/// Outward unit normal of `face` at body-local point `at`.
fn face_normal_local(face: &Face, at: [f64; 3]) -> [f64; 3] {
    match &face.surface {
        SurfaceGeometry::Analytic { surface, transform } => {
            normalize(analytic_normal_local(surface, transform, at))
        }
        SurfaceGeometry::Tessellated { positions, indices } => {
            let mut acc = [0.0f64; 3];
            for tri in indices {
                let a = positions[tri[0] as usize];
                let b = positions[tri[1] as usize];
                let c = positions[tri[2] as usize];
                let e1 = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
                let e2 = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
                acc[0] += e1[1] * e2[2] - e1[2] * e2[1];
                acc[1] += e1[2] * e2[0] - e1[0] * e2[2];
                acc[2] += e1[0] * e2[1] - e1[1] * e2[0];
            }
            normalize(acc)
        }
    }
}

/// Outward normal of an analytic half-space surface at body-local point `at`,
/// computed as the surface-SDF gradient in the surface's local frame and mapped
/// back into the body-local frame by the face transform's rotation. The
/// half-space SDF is positive *outside* the solid, so its gradient is the
/// outward normal.
fn analytic_normal_local(surface: &HalfSpace, transform: &Transform, at: [f64; 3]) -> [f64; 3] {
    // Plane has a constant normal — map it directly (cheaper + exact).
    if let HalfSpace::Plane { normal, .. } = surface {
        return rotate_dir(transform.rotation, [normal[0] as f64, normal[1] as f64, normal[2] as f64]);
    }
    // Curved surface: central-difference the half-space SDF in its local frame.
    let p_local = crate::world_to_local(transform, [at[0] as f32, at[1] as f32, at[2] as f32]);
    let eps = 1e-3f32;
    let sd = |p: [f32; 3]| crate::csg::sdf_halfspace(surface, p) as f64;
    let gx = sd([p_local[0] + eps, p_local[1], p_local[2]]) - sd([p_local[0] - eps, p_local[1], p_local[2]]);
    let gy = sd([p_local[0], p_local[1] + eps, p_local[2]]) - sd([p_local[0], p_local[1] - eps, p_local[2]]);
    let gz = sd([p_local[0], p_local[1], p_local[2] + eps]) - sd([p_local[0], p_local[1], p_local[2] - eps]);
    rotate_dir(transform.rotation, [gx, gy, gz])
}

/// Rotate a direction by a unit quaternion, in f64.
fn rotate_dir(q: [f32; 4], v: [f64; 3]) -> [f64; 3] {
    let qx = q[0] as f64;
    let qy = q[1] as f64;
    let qz = q[2] as f64;
    let qw = q[3] as f64;
    let t = [
        2.0 * (qy * v[2] - qz * v[1]),
        2.0 * (qz * v[0] - qx * v[2]),
        2.0 * (qx * v[1] - qy * v[0]),
    ];
    [
        v[0] + qw * t[0] + qy * t[2] - qz * t[1],
        v[1] + qw * t[1] + qz * t[0] - qx * t[2],
        v[2] + qw * t[2] + qx * t[1] - qy * t[0],
    ]
}

/// Tag every render-mesh **triangle** with the arena index of the B-Rep
/// [`Face`] it lies on. Returns a `Vec<u32>` of length `indices.len() / 3`
/// (one entry per triangle, in triangle order); each entry is a `FaceId.0` or
/// [`FACE_INDEX_SENTINEL`].
///
/// ## Why per-triangle, not per-vertex
///
/// The render mesh runs through planar/developable simplification (see `dc.rs`),
/// which replaces a flat face's dense interior with a minimal ear-clipped
/// polygon built **only from its boundary-ring vertices**. A box therefore has
/// *no* face-interior vertices — every vertex sits on a shared edge or corner —
/// so a vertex cannot be uniquely owned by one of the six faces. A **triangle**,
/// however, has a centroid that lies squarely in the interior of exactly one
/// face, which is the natural granularity for "this clicked triangle is on
/// face N" anyway.
///
/// ## Strategy
///
/// The mesh's per-vertex `contributors` carry only **flat feature ids**
/// (sub-index stripped — see `mdc.rs`), so a box's six faces — all one feature
/// — cannot be told apart by contributor alone. Correlation therefore uses
/// geometry: for each triangle,
///
/// 1. compute its centroid + geometric normal, brought into the body-local
///    frame via `inverse(placement)`;
/// 2. gather candidate faces whose `branch_id.feature_id()` appears among the
///    triangle's vertices' contributors (prunes by feature, the only identity
///    the mesh retains);
/// 3. score each candidate by its absolute surface distance at the centroid
///    plus a normal-misalignment penalty, and pick the minimum. The centroid is
///    face-interior so the distance term alone is usually decisive; the normal
///    term only matters for thin slivers. Centroid distance is the final
///    deterministic tie-break across weaves.
///
/// Triangles with no candidate analytic face (smooth blends, tessellated-only
/// regions) keep the sentinel.
///
/// `normals` is the mesh's per-vertex normal buffer (xyz-interleaved, world
/// frame); it is unused except as a fallback and may be empty.
pub fn correlate_mesh_faces(
    brep: &BRep,
    positions: &[f32],
    _normals: &[f32],
    indices: &[u32],
    contributors: &[u32],
    placement: &Transform,
) -> Vec<u32> {
    let tri_count = indices.len() / 3;
    let mut out = vec![FACE_INDEX_SENTINEL; tri_count];
    if brep.faces.is_empty() {
        return out;
    }

    // Pre-bucket faces by flat feature id and precompute each face's body-local
    // outward normal + centroid.
    let mut faces_by_feature: HashMap<u32, Vec<u32>> = HashMap::new();
    let mut centroids: Vec<[f64; 3]> = Vec::with_capacity(brep.faces.len());
    let mut face_normals: Vec<[f64; 3]> = Vec::with_capacity(brep.faces.len());
    for (k, face) in brep.faces.iter().enumerate() {
        faces_by_feature
            .entry(face.branch_id.feature_id())
            .or_default()
            .push(k as u32);
        let c = face_centroid_local(brep, face);
        centroids.push(c);
        face_normals.push(face_normal_local(face, c));
    }

    let inv = crate::inverse(placement);
    // Distance dominates; the normal term is only a gentle tiebreak for
    // slivers where two faces are near-equidistant at the centroid. A large
    // weight here would *mis*-assign flat-interior triangles whose geometric
    // normal wobbles, so keep it small.
    const NORMAL_WEIGHT: f64 = 0.25;

    for t in 0..tri_count {
        let i0 = indices[t * 3] as usize;
        let i1 = indices[t * 3 + 1] as usize;
        let i2 = indices[t * 3 + 2] as usize;
        let p0 = [positions[i0 * 3], positions[i0 * 3 + 1], positions[i0 * 3 + 2]];
        let p1 = [positions[i1 * 3], positions[i1 * 3 + 1], positions[i1 * 3 + 2]];
        let p2 = [positions[i2 * 3], positions[i2 * 3 + 1], positions[i2 * 3 + 2]];

        // World-space centroid + geometric normal → body-local.
        let cw = [
            (p0[0] + p1[0] + p2[0]) / 3.0,
            (p0[1] + p1[1] + p2[1]) / 3.0,
            (p0[2] + p1[2] + p2[2]) / 3.0,
        ];
        let cl = apply_transform(&inv, cw);
        let e1 = [p1[0] - p0[0], p1[1] - p0[1], p1[2] - p0[2]];
        let e2 = [p2[0] - p0[0], p2[1] - p0[1], p2[2] - p0[2]];
        let nw = [
            e1[1] * e2[2] - e1[2] * e2[1],
            e1[2] * e2[0] - e1[0] * e2[2],
            e1[0] * e2[1] - e1[1] * e2[0],
        ];
        let nr = crate::quat_rotate(inv.rotation, nw);
        let nl = {
            let len = (nr[0] * nr[0] + nr[1] * nr[1] + nr[2] * nr[2]).sqrt();
            if len > 1e-12 {
                [(nr[0] / len) as f64, (nr[1] / len) as f64, (nr[2] / len) as f64]
            } else {
                [0.0; 3]
            }
        };

        // Candidate faces = union of the 3 vertices' contributor features.
        let mut best: Option<(u32, f64)> = None;
        let mut seen_feat = [u32::MAX; 3 * MAX_CONTRIBUTORS];
        let mut seen_len = 0usize;
        for &vi in &[i0, i1, i2] {
            let base = vi * MAX_CONTRIBUTORS;
            for slot in 0..MAX_CONTRIBUTORS {
                let feat = contributors[base + slot];
                if feat == CONTRIBUTOR_SENTINEL {
                    break;
                }
                if seen_feat[..seen_len].contains(&feat) {
                    continue;
                }
                seen_feat[seen_len] = feat;
                seen_len += 1;
                let Some(cands) = faces_by_feature.get(&feat) else {
                    continue;
                };
                for &fk in cands {
                    let face = &brep.faces[fk as usize];
                    let SurfaceGeometry::Analytic { surface, transform } = &face.surface else {
                        continue;
                    };
                    let p_surf = crate::world_to_local(transform, cl);
                    let dist = crate::csg::sdf_halfspace(surface, p_surf).abs() as f64;
                    let fnl = face_normals[fk as usize];
                    let dot = nl[0] * fnl[0] + nl[1] * fnl[1] + nl[2] * fnl[2];
                    let align = 1.0 - dot; // [0, 2]
                    let mut score = dist + NORMAL_WEIGHT * align;
                    score += dist2(centroids[fk as usize], cl) * 1e-9;
                    if best.is_none_or(|(_, bs)| score < bs) {
                        best = Some((fk, score));
                    }
                }
            }
        }
        if let Some((fk, _)) = best {
            out[t] = fk;
        }
    }

    out
}

/// Apply a rigid + uniform-scale transform to a point: `R·(s·p) + t`.
fn apply_transform(t: &Transform, p: [f32; 3]) -> [f32; 3] {
    let s = t.scale[0];
    let scaled = [p[0] * s, p[1] * s, p[2] * s];
    let r = crate::quat_rotate(t.rotation, scaled);
    [r[0] + t.translate[0], r[1] + t.translate[1], r[2] + t.translate[2]]
}

fn dist2(a: [f64; 3], b: [f32; 3]) -> f64 {
    let dx = a[0] - b[0] as f64;
    let dy = a[1] - b[1] as f64;
    let dz = a[2] - b[2] as f64;
    dx * dx + dy * dy + dz * dz
}
