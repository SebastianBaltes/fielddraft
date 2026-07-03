//! Per-face parametric tessellation of a woven B-Rep (Meshing Phase 2,
//! Option B in docs/MESHING-EXPORT-ANALYSIS.md §4).
//!
//! The route to a *provably* watertight export mesh: every B-Rep edge
//! curve is discretised **exactly once** (chord-tolerance driven), and
//! both adjacent faces consume the identical vertex chain — shared
//! vertices by construction, so the shell cannot crack along an edge.
//! Face interiors are triangulated on their analytic carrier
//! (plane / cylinder / cone / sphere / torus), with interior points
//! lifted onto the exact surface, so the deviation is bounded by the
//! sag of the chosen step, not by a voxel grid.
//!
//! Scope (Phase 2a): Plane, InfiniteCylinder, Cone (as rotational
//! surfaces about the local y-axis), plus fully-closed Sphere/Torus
//! faces (structured grids). Anything else — RevolvedProfile,
//! Tessellated (spline extrude walls, TPMS), trimmed sphere/torus
//! patches, multi-winding cylinder patches — returns `Err`, and the
//! export session falls back to the generic MDC pass for that body.
//! The caller is expected to run the mesh validator afterwards; the
//! parametric result must EARN the export (watertight + measured
//! deviation), it is never trusted blindly.

use std::collections::HashMap;

use crate::brep::edge_sampling::sample_edge_dense;
use crate::brep::{BRep, Face, Loop, SurfaceGeometry, VertexId};
use crate::csg::MAX_CONTRIBUTORS;
use crate::dc::{Mesh, CONTRIBUTOR_SENTINEL};
use fielddraft_protocol::{HalfSpace, Transform};

/// Dense per-Bézier-span sampling before chord decimation. A quarter
/// circle span at 64 samples has ~0.025 rad steps — far below any
/// realistic export tolerance; Douglas-Peucker prunes the excess.
const EDGE_SAMPLES_PER_SPAN: usize = 64;

/// Cap on refinement passes (each pass halves the angular extent of
/// offending triangles, so 24 passes cover any realistic patch).
const MAX_REFINE_PASSES: usize = 24;

// ─── small f64 vector / transform helpers ───────────────────────────────────

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn norm(a: [f64; 3]) -> f64 {
    dot(a, a).sqrt()
}

fn quat_rotate_f64(q: [f64; 4], v: [f64; 3]) -> [f64; 3] {
    let [qx, qy, qz, qw] = q;
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

fn quat_f64(t: &Transform) -> [f64; 4] {
    [
        t.rotation[0] as f64,
        t.rotation[1] as f64,
        t.rotation[2] as f64,
        t.rotation[3] as f64,
    ]
}

/// local → placed: `T(p) = translate + R(p · scale)`.
fn xf_apply(t: &Transform, p: [f64; 3]) -> [f64; 3] {
    let scaled = [
        p[0] * t.scale[0] as f64,
        p[1] * t.scale[1] as f64,
        p[2] * t.scale[2] as f64,
    ];
    let r = quat_rotate_f64(quat_f64(t), scaled);
    [
        r[0] + t.translate[0] as f64,
        r[1] + t.translate[1] as f64,
        r[2] + t.translate[2] as f64,
    ]
}

/// placed → local: inverse of [`xf_apply`].
fn xf_unapply(t: &Transform, p: [f64; 3]) -> [f64; 3] {
    let q = quat_f64(t);
    let conj = [-q[0], -q[1], -q[2], q[3]];
    let shifted = [
        p[0] - t.translate[0] as f64,
        p[1] - t.translate[1] as f64,
        p[2] - t.translate[2] as f64,
    ];
    let r = quat_rotate_f64(conj, shifted);
    [
        r[0] / t.scale[0] as f64,
        r[1] / t.scale[1] as f64,
        r[2] / t.scale[2] as f64,
    ]
}

fn uniform_scale(t: &Transform) -> Result<f64, String> {
    let [sx, sy, sz] = t.scale;
    if (sx - sy).abs() > 1e-4 || (sy - sz).abs() > 1e-4 {
        return Err(format!("non-uniform face scale [{sx}, {sy}, {sz}]"));
    }
    Ok(sx as f64)
}

// ─── mesh builder ────────────────────────────────────────────────────────────

struct Builder {
    /// Body-local positions (f64 until the final world conversion).
    positions: Vec<[f64; 3]>,
    triangles: Vec<[u32; 3]>,
}

impl Builder {
    fn add_point(&mut self, p: [f64; 3]) -> u32 {
        let id = self.positions.len() as u32;
        self.positions.push(p);
        id
    }

    fn push_tri(&mut self, a: u32, b: u32, c: u32) {
        if a != b && b != c && c != a {
            self.triangles.push([a, b, c]);
        }
    }
}

// ─── entry point ─────────────────────────────────────────────────────────────

/// Tessellate a woven, closed B-Rep into a watertight-by-construction
/// triangle mesh at chord tolerance `tol_mm`. Returns the mesh in
/// **world** space (the body `placement` is applied). `Err` carries the
/// reason the parametric route is not available for this body — the
/// caller falls back to the generic mesher.
pub fn tessellate_brep(brep: &BRep, placement: &Transform, tol_mm: f64) -> Result<Mesh, String> {
    if brep.faces.is_empty() {
        return Err("empty B-Rep (no faces)".into());
    }
    if brep.solids.is_empty() || brep.shells.iter().any(|s| !s.is_closed) {
        return Err("B-Rep has no closed solid shell".into());
    }
    let tol = tol_mm.max(1e-4);

    let mut builder = Builder { positions: Vec::new(), triangles: Vec::new() };

    // ── shared topological vertices ─────────────────────────────
    let mut vertex_vid: HashMap<u32, u32> = HashMap::new();
    for (i, v) in brep.vertices.iter().enumerate() {
        let vid = builder.add_point(v.position);
        vertex_vid.insert(i as u32, vid);
    }

    // ── every edge discretised exactly once ─────────────────────
    // Forward orientation (start → end); faces walk it reversed via the
    // loop flag. Closed edges keep first == last as ONE vid.
    let mut edge_chain: Vec<Vec<u32>> = Vec::with_capacity(brep.edges.len());
    for edge in &brep.edges {
        let mut pts = sample_edge_dense(&edge.geometry, EDGE_SAMPLES_PER_SPAN);
        if pts.len() < 2 {
            return Err("degenerate edge (fewer than 2 samples)".into());
        }
        // Snap endpoints onto the shared topological vertices so all
        // edges meeting at a corner reference the identical position.
        if let Some(VertexId(s)) = edge.start {
            pts[0] = brep.vertices[s as usize].position;
        }
        if let Some(VertexId(e)) = edge.end {
            let n = pts.len();
            pts[n - 1] = brep.vertices[e as usize].position;
        }
        // Edge chord budget: face triangles lean on the boundary
        // discretisation (band strips consume it directly, and phase
        // offsets between the two band rings stretch a triangle's
        // angular span to ~2 segments), so the per-segment sag budget
        // is a fraction of the face tolerance.
        let kept = douglas_peucker(&pts, tol / 6.0);

        let closed = edge.start.is_none() && edge.end.is_none();
        let mut vids: Vec<u32> = Vec::with_capacity(kept.len());
        for (k, &pi) in kept.iter().enumerate() {
            let is_first = k == 0;
            let is_last = k == kept.len() - 1;
            let vid = if is_first {
                match edge.start {
                    Some(VertexId(s)) => vertex_vid[&s],
                    None => builder.add_point(pts[pi]),
                }
            } else if is_last {
                match edge.end {
                    Some(VertexId(e)) => vertex_vid[&e],
                    None => vids[0], // closed edge: last == first
                }
            } else {
                builder.add_point(pts[pi])
            };
            vids.push(vid);
        }
        if closed && vids.len() < 4 {
            return Err("closed edge decimated below a triangle".into());
        }
        edge_chain.push(vids);
    }

    // ── per-face tessellation ───────────────────────────────────
    for (fi, face) in brep.faces.iter().enumerate() {
        tessellate_face(brep, face, &edge_chain, tol, &mut builder)
            .map_err(|e| format!("face {fi}: {e}"))?;
    }

    // ── assemble the world-space mesh ───────────────────────────
    let n = builder.positions.len();
    let mut positions: Vec<f32> = Vec::with_capacity(n * 3);
    for p in &builder.positions {
        let w = xf_apply(placement, *p);
        positions.extend_from_slice(&[w[0] as f32, w[1] as f32, w[2] as f32]);
    }
    let mut indices: Vec<u32> = Vec::with_capacity(builder.triangles.len() * 3);
    for t in &builder.triangles {
        indices.extend_from_slice(t);
    }

    // Area-weighted smoothed vertex normals — cosmetic only (STL/3MF
    // recompute facet normals from winding), but keeps the Mesh layout
    // complete for any downstream consumer.
    let mut normals = vec![0.0f32; n * 3];
    for t in &builder.triangles {
        let v = |i: u32| {
            let b = i as usize * 3;
            [
                positions[b] as f64,
                positions[b + 1] as f64,
                positions[b + 2] as f64,
            ]
        };
        let (a, b, c) = (v(t[0]), v(t[1]), v(t[2]));
        let fnorm = cross(sub(b, a), sub(c, a));
        for &i in t {
            let base = i as usize * 3;
            normals[base] += fnorm[0] as f32;
            normals[base + 1] += fnorm[1] as f32;
            normals[base + 2] += fnorm[2] as f32;
        }
    }
    for i in 0..n {
        let b = i * 3;
        let len =
            (normals[b] * normals[b] + normals[b + 1] * normals[b + 1] + normals[b + 2] * normals[b + 2])
                .sqrt();
        if len > 1e-20 {
            normals[b] /= len;
            normals[b + 1] /= len;
            normals[b + 2] /= len;
        }
    }

    Ok(Mesh {
        positions,
        normals,
        indices,
        classifications: vec![1u8; n],
        contributors: vec![CONTRIBUTOR_SENTINEL; n * MAX_CONTRIBUTORS],
        face_index: Vec::new(),
    })
}

// ─── edge decimation ─────────────────────────────────────────────────────────

/// Douglas-Peucker on a 3D polyline: returns the kept indices (always
/// including both endpoints, in order).
fn douglas_peucker(pts: &[[f64; 3]], tol: f64) -> Vec<usize> {
    let n = pts.len();
    if n <= 2 {
        return (0..n).collect();
    }
    let mut keep = vec![false; n];
    keep[0] = true;
    keep[n - 1] = true;
    let mut stack = vec![(0usize, n - 1)];
    while let Some((lo, hi)) = stack.pop() {
        if hi <= lo + 1 {
            continue;
        }
        let a = pts[lo];
        let b = pts[hi];
        let ab = sub(b, a);
        let ab_len2 = dot(ab, ab);
        let mut worst = 0.0;
        let mut worst_i = lo;
        for (i, p) in pts.iter().enumerate().take(hi).skip(lo + 1) {
            let ap = sub(*p, a);
            let d2 = if ab_len2 > 1e-30 {
                let t = (dot(ap, ab) / ab_len2).clamp(0.0, 1.0);
                let proj = [a[0] + ab[0] * t, a[1] + ab[1] * t, a[2] + ab[2] * t];
                let dv = sub(*p, proj);
                dot(dv, dv)
            } else {
                dot(ap, ap)
            };
            if d2 > worst {
                worst = d2;
                worst_i = i;
            }
        }
        if worst > tol * tol {
            keep[worst_i] = true;
            stack.push((lo, worst_i));
            stack.push((worst_i, hi));
        }
    }
    (0..n).filter(|&i| keep[i]).collect()
}

// ─── loops → vertex rings ────────────────────────────────────────────────────

/// Concatenate a loop's oriented edge chains into one closed ring of
/// global vids (first vertex NOT repeated at the end).
fn loop_ring(lp: &Loop, edge_chain: &[Vec<u32>]) -> Result<Vec<u32>, String> {
    let mut ring: Vec<u32> = Vec::new();
    for &(eid, reversed) in &lp.edges {
        let chain = &edge_chain[eid.0 as usize];
        let mut part: Vec<u32> = if reversed {
            chain.iter().rev().copied().collect()
        } else {
            chain.clone()
        };
        // Closed single-edge loop: drop the duplicated closing vid.
        if part.first() == part.last() && part.len() > 1 {
            part.pop();
        }
        if let Some(&last) = ring.last() {
            if part.first() == Some(&last) {
                part.remove(0);
            } else {
                return Err("loop edges do not chain head-to-tail".into());
            }
        }
        ring.extend(part);
    }
    // The final edge closes back onto the ring start.
    if ring.first() == ring.last() && ring.len() > 1 {
        ring.pop();
    }
    if ring.len() < 3 {
        return Err("ring has fewer than 3 vertices".into());
    }
    Ok(ring)
}

// ─── face dispatch ───────────────────────────────────────────────────────────

fn tessellate_face(
    brep: &BRep,
    face: &Face,
    edge_chain: &[Vec<u32>],
    tol: f64,
    builder: &mut Builder,
) -> Result<(), String> {
    let SurfaceGeometry::Analytic { surface, transform } = &face.surface else {
        return Err("tessellated (non-analytic) carrier".into());
    };
    let scale = uniform_scale(transform)?;

    // Fully-closed faces (no boundary loops at all): lone sphere/torus.
    let unbounded = face.outer.edges.is_empty() && face.inner.is_empty();
    if unbounded {
        return match surface {
            HalfSpace::Sphere { radius } => {
                grid_sphere(*radius as f64, transform, scale, tol, builder);
                Ok(())
            }
            HalfSpace::Torus { major_radius, minor_radius } => {
                grid_torus(
                    *major_radius as f64,
                    *minor_radius as f64,
                    transform,
                    scale,
                    tol,
                    builder,
                );
                Ok(())
            }
            _ => Err("unbounded face on a non-closed carrier".into()),
        };
    }

    if !face.outer.is_connected_ring(&brep.edges)
        || face.inner.iter().any(|l| !l.is_connected_ring(&brep.edges))
    {
        return Err("loop is not a connected ring".into());
    }

    let outer = loop_ring(&face.outer, edge_chain)?;
    let inners: Vec<Vec<u32>> = face
        .inner
        .iter()
        .map(|l| loop_ring(l, edge_chain))
        .collect::<Result<_, _>>()?;

    match surface {
        HalfSpace::Plane { normal, .. } => {
            tess_plane(*normal, transform, &outer, &inners, builder)
        }
        HalfSpace::InfiniteCylinder { radius } => {
            let r = *radius as f64;
            tess_rotational(RotationalKind::Cylinder { r }, transform, scale, &outer, &inners, tol, builder)
        }
        HalfSpace::Cone { half_angle } => tess_rotational(
            RotationalKind::Cone { tan_half: (*half_angle as f64).tan() },
            transform,
            scale,
            &outer,
            &inners,
            tol,
            builder,
        ),
        HalfSpace::Sphere { .. } | HalfSpace::Torus { .. } => {
            Err("trimmed sphere/torus patch (Phase 2a limitation)".into())
        }
        HalfSpace::Gyroid { .. } => Err("gyroid carrier".into()),
        HalfSpace::RevolvedProfile { .. } => {
            Err("revolved-profile carrier (Phase 2a limitation)".into())
        }
    }
}

// ─── planar faces ────────────────────────────────────────────────────────────

fn orthonormal_basis_f64(n: [f64; 3]) -> ([f64; 3], [f64; 3]) {
    let up = if n[1].abs() < 0.9 { [0.0, 1.0, 0.0] } else { [1.0, 0.0, 0.0] };
    let mut a = cross(up, n);
    let l = norm(a);
    a = [a[0] / l, a[1] / l, a[2] / l];
    let b = cross(n, a);
    (a, b)
}

fn tess_plane(
    normal: [f32; 3],
    transform: &Transform,
    outer: &[u32],
    inners: &[Vec<u32>],
    builder: &mut Builder,
) -> Result<(), String> {
    let mut n = [normal[0] as f64, normal[1] as f64, normal[2] as f64];
    let l = norm(n);
    if l < 1e-12 {
        return Err("degenerate plane normal".into());
    }
    n = [n[0] / l, n[1] / l, n[2] / l];
    let (a_axis, b_axis) = orthonormal_basis_f64(n);

    // Project all ring points into the plane's (u,v) chart (face-local
    // frame so the carrier is exactly the analytic half-space).
    let project = |vid: u32, builder: &Builder| -> [f64; 2] {
        let p = xf_unapply(transform, builder.positions[vid as usize]);
        [dot(p, a_axis), dot(p, b_axis)]
    };

    let mut vids: Vec<u32> = outer.to_vec();
    let mut uv: Vec<[f64; 2]> = outer.iter().map(|&v| project(v, builder)).collect();
    let mut rings: Vec<(usize, usize)> = vec![(0, outer.len())];
    for ring in inners {
        let s = vids.len();
        vids.extend_from_slice(ring);
        uv.extend(ring.iter().map(|&v| project(v, builder)));
        rings.push((s, ring.len()));
    }

    let tris = triangulate_polygon_with_holes(&uv, &rings)?;
    for [i, j, k] in tris {
        builder.push_tri(vids[i], vids[j], vids[k]);
    }
    Ok(())
}

// ─── rotational faces (cylinder / cone about the local y-axis) ───────────────

enum RotationalKind {
    Cylinder { r: f64 },
    Cone { tan_half: f64 },
}

impl RotationalKind {
    fn radius_at(&self, v: f64) -> f64 {
        match self {
            RotationalKind::Cylinder { r } => *r,
            RotationalKind::Cone { tan_half } => v.abs() * tan_half,
        }
    }
}

/// Max angular step so the *triangle* sag stays ≤ `tol` on radius `r`.
/// The chord-sag formula bounds one edge of step Δθ; a triangle (grid
/// quad diagonal, band diagonal with phase offset) spans up to ~√2×
/// that, and sag grows quadratically — so the per-edge budget is
/// `tol/2`.
fn max_dtheta(r: f64, tol: f64) -> f64 {
    let budget = tol * 0.5;
    if r <= budget {
        return std::f64::consts::FRAC_PI_2;
    }
    (2.0 * (1.0 - budget / r).acos()).clamp(1e-3, std::f64::consts::FRAC_PI_2)
}

fn tess_rotational(
    kind: RotationalKind,
    transform: &Transform,
    _scale: f64,
    outer: &[u32],
    inners: &[Vec<u32>],
    tol: f64,
    builder: &mut Builder,
) -> Result<(), String> {
    // Unwrap every ring into continuous (theta, v).
    let unwrap_ring = |ring: &[u32], builder: &Builder| -> (Vec<[f64; 2]>, f64) {
        let mut out: Vec<[f64; 2]> = Vec::with_capacity(ring.len());
        let mut prev_theta = 0.0_f64;
        for (i, &vid) in ring.iter().enumerate() {
            let p = xf_unapply(transform, builder.positions[vid as usize]);
            let raw = p[2].atan2(p[0]);
            let theta = if i == 0 {
                raw
            } else {
                let mut d = raw - (prev_theta.rem_euclid(2.0 * std::f64::consts::PI));
                // Wrap the step into (−π, π].
                while d > std::f64::consts::PI {
                    d -= 2.0 * std::f64::consts::PI;
                }
                while d <= -std::f64::consts::PI {
                    d += 2.0 * std::f64::consts::PI;
                }
                prev_theta + d
            };
            prev_theta = theta;
            out.push([theta, p[1]]);
        }
        // Winding: the closing step back to the ring start.
        let first_raw = out[0][0].rem_euclid(2.0 * std::f64::consts::PI);
        let last_mod = prev_theta.rem_euclid(2.0 * std::f64::consts::PI);
        let mut close = first_raw - last_mod;
        while close > std::f64::consts::PI {
            close -= 2.0 * std::f64::consts::PI;
        }
        while close <= -std::f64::consts::PI {
            close += 2.0 * std::f64::consts::PI;
        }
        let total = prev_theta + close - out[0][0];
        let winding = (total / (2.0 * std::f64::consts::PI)).round();
        (out, winding)
    };

    let (outer_tv, outer_w) = unwrap_ring(outer, builder);
    let inner_unwrapped: Vec<(Vec<[f64; 2]>, f64)> =
        inners.iter().map(|r| unwrap_ring(r, builder)).collect();

    let r_max = outer_tv
        .iter()
        .chain(inner_unwrapped.iter().flat_map(|(tv, _)| tv.iter()))
        .map(|tv| kind.radius_at(tv[1]))
        .fold(0.0_f64, f64::max);
    if r_max <= 0.0 {
        return Err("rotational face with zero radius".into());
    }
    let du_max = max_dtheta(r_max, tol);

    if outer_w.abs() < 0.5 && inner_unwrapped.iter().all(|(_, w)| w.abs() < 0.5) {
        // ── simple patch: unwrapped simple polygon (+ holes) ──────
        // Align each hole's 2π-branch with the outer ring.
        let outer_mid = mid_theta(&outer_tv);
        let mut vids: Vec<u32> = outer.to_vec();
        let mut uv: Vec<[f64; 2]> = outer_tv.iter().map(|tv| [tv[0] * r_max, tv[1]]).collect();
        let mut rings: Vec<(usize, usize)> = vec![(0, outer.len())];
        for ((ring, (tv, _)), _) in inners.iter().zip(&inner_unwrapped).zip(0..) {
            let shift = ((outer_mid - mid_theta(tv)) / (2.0 * std::f64::consts::PI)).round()
                * 2.0
                * std::f64::consts::PI;
            let s = vids.len();
            vids.extend_from_slice(ring);
            uv.extend(tv.iter().map(|t| [(t[0] + shift) * r_max, t[1]]));
            rings.push((s, ring.len()));
        }
        let tris = triangulate_polygon_with_holes(&uv, &rings)?;

        // Refinement setup: per-face param map + boundary edge set.
        let mut theta_of: HashMap<u32, f64> = HashMap::new();
        let mut v_of: HashMap<u32, f64> = HashMap::new();
        for (i, &vid) in vids.iter().enumerate() {
            theta_of.insert(vid, uv[i][0] / r_max);
            v_of.insert(vid, uv[i][1]);
        }
        let mut boundary: std::collections::HashSet<(u32, u32)> = std::collections::HashSet::new();
        for &(s, len) in &rings {
            for i in 0..len {
                let a = vids[s + i];
                let b = vids[s + (i + 1) % len];
                boundary.insert(if a < b { (a, b) } else { (b, a) });
            }
        }

        let mut tris: Vec<[u32; 3]> = tris
            .into_iter()
            .map(|[i, j, k]| [vids[i], vids[j], vids[k]])
            .collect();
        refine_rotational(
            &kind, transform, du_max, &mut tris, &mut theta_of, &mut v_of, &boundary, builder,
        );
        for [a, b, c] in tris {
            builder.push_tri(a, b, c);
        }
        Ok(())
    } else if outer_w.abs() > 0.5 && inner_unwrapped.len() == 1 && inner_unwrapped[0].1.abs() > 0.5
    {
        // ── full band between two once-winding rings ──────────────
        let (inner_ring, (inner_tv, _)) = (&inners[0], &inner_unwrapped[0]);
        strip_band(
            &kind, transform, outer, &outer_tv, inner_ring, inner_tv, builder,
        )
    } else {
        Err(format!(
            "unsupported winding configuration (outer {outer_w}, {} inner)",
            inner_unwrapped.len()
        ))
    }
}

fn mid_theta(tv: &[[f64; 2]]) -> f64 {
    let (min, max) = tv
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), t| {
            (lo.min(t[0]), hi.max(t[0]))
        });
    (min + max) * 0.5
}

/// Lift a rotational param point back to the body-local frame.
fn lift_rotational(kind: &RotationalKind, transform: &Transform, theta: f64, v: f64) -> [f64; 3] {
    let r = kind.radius_at(v);
    xf_apply(transform, [r * theta.cos(), v, r * theta.sin()])
}

/// Longest-edge-style conformal refinement: split every non-boundary
/// triangle edge whose angular extent exceeds `du_max`, lifting the
/// midpoints onto the exact carrier. Splits are memoised per undirected
/// edge, so adjacent triangles stay conforming; boundary edges (shared
/// with neighbour faces) are never split.
#[allow(clippy::too_many_arguments)]
fn refine_rotational(
    kind: &RotationalKind,
    transform: &Transform,
    du_max: f64,
    tris: &mut Vec<[u32; 3]>,
    theta_of: &mut HashMap<u32, f64>,
    v_of: &mut HashMap<u32, f64>,
    boundary: &std::collections::HashSet<(u32, u32)>,
    builder: &mut Builder,
) {
    for _ in 0..MAX_REFINE_PASSES {
        // Collect the set of edges to split this pass.
        let mut split_mid: HashMap<(u32, u32), u32> = HashMap::new();
        for tri in tris.iter() {
            for e in 0..3 {
                let a = tri[e];
                let b = tri[(e + 1) % 3];
                let key = if a < b { (a, b) } else { (b, a) };
                if boundary.contains(&key) || split_mid.contains_key(&key) {
                    continue;
                }
                let dth = (theta_of[&a] - theta_of[&b]).abs();
                if dth > du_max {
                    let mt = (theta_of[&a] + theta_of[&b]) * 0.5;
                    let mv = (v_of[&a] + v_of[&b]) * 0.5;
                    let vid = builder.add_point(lift_rotational(kind, transform, mt, mv));
                    theta_of.insert(vid, mt);
                    v_of.insert(vid, mv);
                    split_mid.insert(key, vid);
                }
            }
        }
        if split_mid.is_empty() {
            return;
        }

        // Re-triangulate every triangle by its split pattern.
        let mut next: Vec<[u32; 3]> = Vec::with_capacity(tris.len() + split_mid.len() * 2);
        for tri in tris.iter() {
            let [a, b, c] = *tri;
            let mid = |x: u32, y: u32| -> Option<u32> {
                split_mid.get(&if x < y { (x, y) } else { (y, x) }).copied()
            };
            match (mid(a, b), mid(b, c), mid(c, a)) {
                (None, None, None) => next.push([a, b, c]),
                (Some(m), None, None) => {
                    next.push([a, m, c]);
                    next.push([m, b, c]);
                }
                (None, Some(m), None) => {
                    next.push([b, m, a]);
                    next.push([m, c, a]);
                }
                (None, None, Some(m)) => {
                    next.push([c, m, b]);
                    next.push([m, a, b]);
                }
                (Some(mab), Some(mbc), None) => {
                    next.push([b, mbc, mab]);
                    next.push([a, mab, mbc]);
                    next.push([a, mbc, c]);
                }
                (None, Some(mbc), Some(mca)) => {
                    next.push([c, mca, mbc]);
                    next.push([b, mbc, mca]);
                    next.push([b, mca, a]);
                }
                (Some(mab), None, Some(mca)) => {
                    next.push([a, mab, mca]);
                    next.push([c, mca, mab]);
                    next.push([c, mab, b]);
                }
                (Some(mab), Some(mbc), Some(mca)) => {
                    next.push([a, mab, mca]);
                    next.push([b, mbc, mab]);
                    next.push([c, mca, mbc]);
                    next.push([mab, mbc, mca]);
                }
            }
        }
        *tris = next;
    }
}

/// Triangulate the band between two rings that each wind the axis once
/// (a cylinder/cone barrel between two rim curves). Both rings are
/// normalised to ascending theta and merged with a two-pointer sweep —
/// this consumes the rings' own (chord-tolerance) discretisations, so
/// no interior refinement is needed: the surface is ruled in `v`.
fn strip_band(
    kind: &RotationalKind,
    _transform: &Transform,
    outer: &[u32],
    outer_tv: &[[f64; 2]],
    inner: &[u32],
    inner_tv: &[[f64; 2]],
    builder: &mut Builder,
) -> Result<(), String> {
    let _ = kind;
    let two_pi = 2.0 * std::f64::consts::PI;

    // Normalise a ring to ascending theta over one period. A descending
    // ring is walked in reverse ORDER (the theta values stay in the same
    // physical angular coordinate — both band rings must share it, so
    // negating would mirror one ring against the other).
    let normalise = |ring: &[u32], tv: &[[f64; 2]]| -> Result<Vec<(f64, u32)>, String> {
        let ascending = tv[tv.len() - 1][0] >= tv[0][0];
        let mut chain: Vec<(f64, u32)> = if ascending {
            ring.iter().zip(tv).map(|(&vid, t)| (t[0], vid)).collect()
        } else {
            ring.iter()
                .zip(tv)
                .rev()
                .map(|(&vid, t)| (t[0], vid))
                .collect()
        };
        // Shift so the chain starts at theta ∈ [0, 2π).
        let base = chain[0].0;
        for c in chain.iter_mut() {
            c.0 -= base;
        }
        // Monotonicity check (small numeric backsteps tolerated).
        for (i, w) in chain.windows(2).enumerate() {
            if w[1].0 < w[0].0 - 1e-9 {
                return Err(format!(
                    "band ring is not monotone in theta at {i}: {:.4} -> {:.4} (ring len {}, first {:.4}, last {:.4})",
                    w[0].0,
                    w[1].0,
                    chain.len(),
                    chain[0].0,
                    chain[chain.len() - 1].0
                ));
            }
        }
        Ok(chain)
    };

    let a = normalise(outer, outer_tv)?;
    let b = normalise(inner, inner_tv)?;

    // Align B's phase with A (both start near theta 0 after their own
    // shifts; residual phase differences are fine — the sweep is cyclic).
    // Extend both cyclically with their first point at +2π.
    let mut ea: Vec<(f64, u32)> = a.clone();
    ea.push((a[0].0 + two_pi, a[0].1));
    let mut eb: Vec<(f64, u32)> = b.clone();
    eb.push((b[0].0 + two_pi, b[0].1));

    // Two-pointer sweep: always advance the chain whose NEXT theta is
    // smaller, emitting one triangle per step.
    let (mut i, mut j) = (0usize, 0usize);
    while i < ea.len() - 1 || j < eb.len() - 1 {
        let next_a = if i < ea.len() - 1 { ea[i + 1].0 } else { f64::INFINITY };
        let next_b = if j < eb.len() - 1 { eb[j + 1].0 } else { f64::INFINITY };
        if next_a <= next_b {
            builder.push_tri(ea[i].1, ea[i + 1].1, eb[j].1);
            i += 1;
        } else {
            builder.push_tri(ea[i].1, eb[j + 1].1, eb[j].1);
            j += 1;
        }
    }
    Ok(())
}

// ─── closed carriers (lone sphere / torus) ───────────────────────────────────

fn grid_sphere(radius: f64, transform: &Transform, _scale: f64, tol: f64, builder: &mut Builder) {
    let r = radius;
    let du = max_dtheta(r, tol);
    let n_theta = ((2.0 * std::f64::consts::PI / du).ceil() as usize).max(8);
    let n_phi = ((std::f64::consts::PI / du).ceil() as usize).max(4);

    let north = builder.add_point(xf_apply(transform, [0.0, r, 0.0]));
    let south = builder.add_point(xf_apply(transform, [0.0, -r, 0.0]));
    // Interior latitude rows (phi from +pole toward −pole).
    let mut rows: Vec<Vec<u32>> = Vec::with_capacity(n_phi - 1);
    for pi in 1..n_phi {
        let phi = std::f64::consts::PI * pi as f64 / n_phi as f64;
        let (y, ring_r) = (r * phi.cos(), r * phi.sin());
        let mut row = Vec::with_capacity(n_theta);
        for ti in 0..n_theta {
            let th = 2.0 * std::f64::consts::PI * ti as f64 / n_theta as f64;
            row.push(builder.add_point(xf_apply(
                transform,
                [ring_r * th.cos(), y, ring_r * th.sin()],
            )));
        }
        rows.push(row);
    }
    // Pole fans + quad strips.
    for t in 0..n_theta {
        let t2 = (t + 1) % n_theta;
        builder.push_tri(north, rows[0][t2], rows[0][t]);
        let last = rows.len() - 1;
        builder.push_tri(south, rows[last][t], rows[last][t2]);
    }
    for p in 0..rows.len() - 1 {
        for t in 0..n_theta {
            let t2 = (t + 1) % n_theta;
            builder.push_tri(rows[p][t], rows[p][t2], rows[p + 1][t]);
            builder.push_tri(rows[p][t2], rows[p + 1][t2], rows[p + 1][t]);
        }
    }
}

fn grid_torus(
    major: f64,
    minor: f64,
    transform: &Transform,
    _scale: f64,
    tol: f64,
    builder: &mut Builder,
) {
    let n_u = ((2.0 * std::f64::consts::PI / max_dtheta(major + minor, tol)).ceil() as usize).max(8);
    let n_v = ((2.0 * std::f64::consts::PI / max_dtheta(minor, tol)).ceil() as usize).max(6);

    let mut grid: Vec<Vec<u32>> = Vec::with_capacity(n_u);
    for ui in 0..n_u {
        let u = 2.0 * std::f64::consts::PI * ui as f64 / n_u as f64;
        let mut row = Vec::with_capacity(n_v);
        for vi in 0..n_v {
            let v = 2.0 * std::f64::consts::PI * vi as f64 / n_v as f64;
            let ring_r = major + minor * v.cos();
            row.push(builder.add_point(xf_apply(
                transform,
                [ring_r * u.cos(), minor * v.sin(), ring_r * u.sin()],
            )));
        }
        grid.push(row);
    }
    for u in 0..n_u {
        let u2 = (u + 1) % n_u;
        for v in 0..n_v {
            let v2 = (v + 1) % n_v;
            builder.push_tri(grid[u][v], grid[u2][v], grid[u][v2]);
            builder.push_tri(grid[u2][v], grid[u2][v2], grid[u][v2]);
        }
    }
}

// ─── 2D polygon triangulation (earcut with hole elimination) ─────────────────

fn signed_area(uv: &[[f64; 2]], ring: &[usize]) -> f64 {
    let mut a = 0.0;
    for i in 0..ring.len() {
        let p = uv[ring[i]];
        let q = uv[ring[(i + 1) % ring.len()]];
        a += p[0] * q[1] - q[0] * p[1];
    }
    a * 0.5
}

/// Triangulate a polygon with holes given as index ranges into `uv`
/// (`rings[0]` is the outer ring, the rest are holes). Returns triangles
/// as indices into `uv`, CCW in the (u,v) chart.
fn triangulate_polygon_with_holes(
    uv: &[[f64; 2]],
    rings: &[(usize, usize)],
) -> Result<Vec<[usize; 3]>, String> {
    // Outer CCW, holes CW.
    let ring_indices = |&(s, len): &(usize, usize)| -> Vec<usize> { (s..s + len).collect() };
    let mut outer: Vec<usize> = ring_indices(&rings[0]);
    if signed_area(uv, &outer) < 0.0 {
        outer.reverse();
    }
    let mut holes: Vec<Vec<usize>> = Vec::new();
    for r in &rings[1..] {
        let mut h = ring_indices(r);
        if signed_area(uv, &h) > 0.0 {
            h.reverse();
        }
        holes.push(h);
    }
    // Process holes by descending max-x so each bridge only has to look
    // at the (growing) outer ring — the standard earcut order.
    holes.sort_by(|a, b| {
        let ax = a.iter().map(|&i| uv[i][0]).fold(f64::NEG_INFINITY, f64::max);
        let bx = b.iter().map(|&i| uv[i][0]).fold(f64::NEG_INFINITY, f64::max);
        bx.partial_cmp(&ax).unwrap_or(std::cmp::Ordering::Equal)
    });
    for hole in &holes {
        bridge_hole(uv, &mut outer, hole)?;
    }
    earcut(uv, &outer)
}

/// Splice `hole` into `outer` through a mutually-visible vertex pair
/// (rightmost hole vertex → horizontal ray → visible outer vertex).
fn bridge_hole(uv: &[[f64; 2]], outer: &mut Vec<usize>, hole: &[usize]) -> Result<(), String> {
    // Rightmost hole vertex M.
    let (m_pos, _) = hole
        .iter()
        .enumerate()
        .max_by(|a, b| {
            uv[*a.1][0]
                .partial_cmp(&uv[*b.1][0])
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .ok_or("empty hole ring")?;
    let m = hole[m_pos];
    let mp = uv[m];

    // Closest intersection of the +x ray from M with outer edges.
    let mut best: Option<(f64, usize)> = None; // (x of intersection, edge start index in outer)
    for i in 0..outer.len() {
        let p = uv[outer[i]];
        let q = uv[outer[(i + 1) % outer.len()]];
        if (p[1] > mp[1]) == (q[1] > mp[1]) {
            continue; // edge does not straddle the ray's y
        }
        let t = (mp[1] - p[1]) / (q[1] - p[1]);
        let x = p[0] + t * (q[0] - p[0]);
        if x >= mp[0] - 1e-12 && best.map(|(bx, _)| x < bx).unwrap_or(true) {
            best = Some((x, i));
        }
    }
    let (ix, edge_i) = best.ok_or("hole is outside the outer ring")?;

    // Candidate visible vertex: the endpoint of the intersected edge with
    // the larger x. If any reflex outer vertex lies inside triangle
    // (M, I, C), take the one with the smallest angle to the ray instead
    // (standard earcut `findHoleBridge` logic, simplified).
    let e0 = outer[edge_i];
    let e1 = outer[(edge_i + 1) % outer.len()];
    let mut cand_idx = if uv[e0][0] > uv[e1][0] { edge_i } else { (edge_i + 1) % outer.len() };
    let c = uv[outer[cand_idx]];
    let tri = [mp, [ix, mp[1]], c];
    let mut best_metric = f64::INFINITY;
    for (i, &ov) in outer.iter().enumerate() {
        let p = uv[ov];
        if p[0] < mp[0] || p[0] > ix.max(c[0]) {
            continue;
        }
        if point_in_triangle(p, tri[0], tri[1], tri[2]) {
            // tan of the angle between the ray and M→P: smaller = closer
            // to the ray = safer bridge.
            let dx = p[0] - mp[0];
            let dy = (p[1] - mp[1]).abs();
            if dx > 1e-12 {
                let metric = dy / dx;
                if metric < best_metric {
                    best_metric = metric;
                    cand_idx = i;
                }
            }
        }
    }

    // Splice: outer[..=cand], M..hole cycle..M, cand, outer[cand+1..]
    let n = hole.len();
    let mut insertion: Vec<usize> = Vec::with_capacity(n + 2);
    for k in 0..=n {
        insertion.push(hole[(m_pos + k) % n]);
    }
    insertion.push(outer[cand_idx]);
    outer.splice(cand_idx + 1..cand_idx + 1, insertion);
    Ok(())
}

fn point_in_triangle(p: [f64; 2], a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> bool {
    let s = |p1: [f64; 2], p2: [f64; 2], p3: [f64; 2]| {
        (p1[0] - p3[0]) * (p2[1] - p3[1]) - (p2[0] - p3[0]) * (p1[1] - p3[1])
    };
    let d1 = s(p, a, b);
    let d2 = s(p, b, c);
    let d3 = s(p, c, a);
    let has_neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
    let has_pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
    !(has_neg && has_pos)
}

/// Plain ear clipping of a (possibly slit-bridged) simple polygon,
/// CCW input, CCW triangles out.
fn earcut(uv: &[[f64; 2]], ring: &[usize]) -> Result<Vec<[usize; 3]>, String> {
    let mut idx: Vec<usize> = ring.to_vec();
    let mut tris: Vec<[usize; 3]> = Vec::with_capacity(idx.len().saturating_sub(2));
    let cross2 = |o: [f64; 2], a: [f64; 2], b: [f64; 2]| -> f64 {
        (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
    };

    let mut guard = idx.len() * idx.len() + 16;
    let mut i = 0usize;
    while idx.len() > 3 {
        if guard == 0 {
            return Err("ear clipping did not converge".into());
        }
        guard -= 1;
        let n = idx.len();
        let (pa, pb, pc) = (idx[(i + n - 1) % n], idx[i], idx[(i + 1) % n]);
        let (a, b, c) = (uv[pa], uv[pb], uv[pc]);
        let convex = cross2(a, b, c) > 1e-14;
        let mut is_ear = convex;
        if is_ear {
            for &other in idx.iter() {
                if other == pa || other == pb || other == pc {
                    continue;
                }
                let p = uv[other];
                // Skip exact duplicates of the corners (slit bridges).
                if (p[0] - a[0]).abs() < 1e-15 && (p[1] - a[1]).abs() < 1e-15 {
                    continue;
                }
                if (p[0] - b[0]).abs() < 1e-15 && (p[1] - b[1]).abs() < 1e-15 {
                    continue;
                }
                if (p[0] - c[0]).abs() < 1e-15 && (p[1] - c[1]).abs() < 1e-15 {
                    continue;
                }
                if point_in_triangle(p, a, b, c) {
                    is_ear = false;
                    break;
                }
            }
        }
        if is_ear {
            tris.push([pa, pb, pc]);
            idx.remove(i);
            i %= idx.len();
        } else {
            i = (i + 1) % n;
        }
    }
    tris.push([idx[0], idx[1], idx[2]]);
    Ok(tris)
}

// ─── tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh_validate::validate_topology;

    #[test]
    fn douglas_peucker_keeps_endpoints_and_prunes_collinear() {
        let pts: Vec<[f64; 3]> = (0..11).map(|i| [i as f64, 0.0, 0.0]).collect();
        let kept = douglas_peucker(&pts, 0.01);
        assert_eq!(kept, vec![0, 10]);
    }

    #[test]
    fn douglas_peucker_respects_tolerance_on_an_arc() {
        let r = 10.0;
        let pts: Vec<[f64; 3]> = (0..=100)
            .map(|i| {
                let t = std::f64::consts::PI * i as f64 / 100.0;
                [r * t.cos(), r * t.sin(), 0.0]
            })
            .collect();
        let kept = douglas_peucker(&pts, 0.05);
        // Every kept segment's chord sag must stay ≤ tolerance-ish.
        for w in kept.windows(2) {
            let a = pts[w[0]];
            let b = pts[w[1]];
            let mid = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0, 0.0];
            let sag = r - (mid[0] * mid[0] + mid[1] * mid[1]).sqrt();
            assert!(sag <= 0.06, "sag {sag}");
        }
        assert!(kept.len() > 10 && kept.len() < 60, "kept {}", kept.len());
    }

    #[test]
    fn earcut_triangulates_a_square_with_hole() {
        let uv = vec![
            // outer CCW
            [0.0, 0.0],
            [10.0, 0.0],
            [10.0, 10.0],
            [0.0, 10.0],
            // hole (given CCW; the bridger flips it)
            [4.0, 4.0],
            [6.0, 4.0],
            [6.0, 6.0],
            [4.0, 6.0],
        ];
        let tris = triangulate_polygon_with_holes(&uv, &[(0, 4), (4, 4)]).unwrap();
        // Area check: square 100 − hole 4 = 96.
        let mut area = 0.0;
        for [a, b, c] in &tris {
            let (p, q, r) = (uv[*a], uv[*b], uv[*c]);
            area += ((q[0] - p[0]) * (r[1] - p[1]) - (r[0] - p[0]) * (q[1] - p[1])) * 0.5;
        }
        assert!((area - 96.0).abs() < 1e-9, "area {area}");
    }

    #[test]
    fn grid_sphere_is_watertight() {
        let mut b = Builder { positions: Vec::new(), triangles: Vec::new() };
        grid_sphere(10.0, &Transform::IDENTITY, 1.0, 0.05, &mut b);
        let mesh = builder_to_mesh(b);
        let v = validate_topology(&mesh);
        assert!(v.watertight, "{v:?}");
        assert_eq!(v.euler_characteristic, 2);
    }

    #[test]
    fn grid_torus_is_watertight_genus_one() {
        let mut b = Builder { positions: Vec::new(), triangles: Vec::new() };
        grid_torus(20.0, 5.0, &Transform::IDENTITY, 1.0, 0.05, &mut b);
        let mesh = builder_to_mesh(b);
        let v = validate_topology(&mesh);
        assert!(v.watertight, "{v:?}");
        assert_eq!(v.euler_characteristic, 0); // genus 1
    }

    fn builder_to_mesh(b: Builder) -> Mesh {
        let n = b.positions.len();
        Mesh {
            positions: b
                .positions
                .iter()
                .flat_map(|p| [p[0] as f32, p[1] as f32, p[2] as f32])
                .collect(),
            normals: vec![0.0; n * 3],
            indices: b.triangles.iter().flatten().copied().collect(),
            classifications: vec![1; n],
            contributors: vec![],
            face_index: Vec::new(),
        }
    }
}
