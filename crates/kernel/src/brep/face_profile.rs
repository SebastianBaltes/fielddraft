//! Derive a 2D sketch profile from a **planar** B-Rep face's boundary.
//!
//! This is the kernel half of "extrude a face" (roadmap: B-Rep elements as an
//! extrude source). Given a planar face and a world-space [`SketchFrame`], it
//! walks the face's outer + inner loops, projects each bounding edge into the
//! frame's 2D `(u, v)` coordinates and rebuilds them as analytic
//! [`SketchSegment`]s — exact `Line` for straight edges, exact `Arc` for
//! circular edges, and a fine polyline of `Line`s as the fallback for a general
//! spline. The result is a [`Sketch2d`] region (outer `Path`, holes subtracted)
//! that the ordinary [`crate::Primitive::Extrude`] machinery sweeps — so a face
//! extrude reuses the whole sketch-extrude pipeline (SDF, frame composition,
//! boolean fold) for free.
//!
//! **Coordinate spaces.** Edge geometry is body-local (DL-56); the caller's
//! `placement` lifts it to world, and the `frame` (also world) projects it to
//! 2D. For a coplanar multi-face selection the caller passes ONE shared frame so
//! every face's region lands in the same `(u, v)` plane and can be unioned.
//!
//! **Curved faces are deliberately out of scope** (first throw): a non-planar
//! face has no single plane / no 2D parameter domain here, so [`face_region_2d`]
//! returns `None` for anything but [`HalfSpace::Plane`]. Thicken/offset of curved
//! faces is a separate future feature.

use fielddraft_protocol::{HalfSpace, Sketch2d, Sketch2dPrimitive, SketchSegment, Transform};

use crate::brep::{BRep, FaceId, Loop, SurfaceGeometry};
use crate::sketch_features::SketchFrame;
use crate::vec3f::dot;

/// Project a planar face's boundary into `frame`'s 2D coordinates and build the
/// filled [`Sketch2d`] region (outer loop minus inner hole loops).
///
/// Returns `None` if the face is not an analytic plane (curved faces deferred)
/// or its outer loop is degenerate (no extrudable area).
pub fn face_region_2d(
    brep: &BRep,
    face_id: FaceId,
    frame: &SketchFrame,
    placement: &Transform,
) -> Option<Sketch2d> {
    // Planar faces only — a curved face has no single (u, v) plane here.
    match &brep.face(face_id).surface {
        SurfaceGeometry::Analytic { surface: HalfSpace::Plane { .. }, .. } => {}
        _ => return None,
    }

    let face = brep.face(face_id);
    // The outer loop may be a weave fallback (unordered, unoriented, holes
    // folded in — ticket oa9h), so it can assemble into SEVERAL rings: the
    // largest by |area| is the profile, every other ring subtracts as a hole.
    let mut rings = loop_rings(brep, &face.outer, frame, placement);
    for hole in &face.inner {
        // Declared hole loops join the ring pool after the first (outer) ring.
        rings.extend(loop_rings(brep, hole, frame, placement));
    }
    // A ring that does not CLOSE is a leftover of duplicate boundary edges (a
    // coplanar seam traced once per branch pair, ticket vyee) — the real ring
    // already consumed one copy, the orphaned twin can never close. Drop it.
    rings.retain(|r| {
        let gap = ring_closing_gap(r);
        gap <= (0.05 * ring_perimeter(r)).max(1e-6)
    });
    if rings.is_empty() {
        return None;
    }
    rings.sort_by(|a, b| ring_area(b).abs().total_cmp(&ring_area(a).abs()));
    // Micro-rings far below the profile scale are tracer noise (the sub-µm
    // corner squiggles a coplanar seam leaves behind), not real holes.
    let min_diag = 1e-4 * ring_diag(&rings[0]);
    rings.retain(|r| ring_diag(r) >= min_diag);
    // Coincident duplicates: a coplanar seam can emit the SAME boundary once
    // per branch pair, so the pool may hold the full ring twice (ticket vyee).
    // Keeping the twin would subtract the profile from itself. Two rings are
    // duplicates when area, extent and centroid all agree within tolerance —
    // distinct same-size holes (a bolt circle) differ in centroid.
    let mut kept: Vec<Ring> = Vec::new();
    for r in rings {
        if !kept.iter().any(|k| rings_coincident(k, &r)) {
            kept.push(r);
        }
    }
    // A "hole" that TOUCHES the outer boundary is a corner squiggle the seam
    // tracer left behind, not a real hole (real holes lie strictly inside the
    // profile). Punching it would slit the extrusion at the rim.
    if kept.len() > 1 {
        let outer_diag = ring_diag(&kept[0]);
        let touch_tol = (1e-3 * outer_diag).max(1e-5);
        let outer_ring = kept.remove(0);
        kept.retain(|r| {
            !r.iter().flatten().any(|p| {
                outer_ring.iter().any(|c| point_near_polyline(*p, c, touch_tol))
            })
        });
        kept.insert(0, outer_ring);
    }

    let mut it = kept.into_iter();
    let outer = ring_to_segments(&it.next()?);
    if outer.is_empty() {
        return None;
    }
    let mut region = Sketch2d::Prim(Sketch2dPrimitive::Path { segments: outer, closed: true });
    for ring in it {
        let hseg = ring_to_segments(&ring);
        if !hseg.is_empty() {
            region = Sketch2d::Difference(
                Box::new(region),
                Box::new(Sketch2d::Prim(Sketch2dPrimitive::Path { segments: hseg, closed: true })),
            );
        }
    }
    Some(region)
}

/// One assembled boundary ring: the ordered, consistently-oriented projected
/// point chains of its edges.
type Ring = Vec<Vec<[f32; 2]>>;

/// Project one boundary loop's edges into the frame and assemble them into
/// closed rings by ENDPOINT PROXIMITY. The weave's loop order/orientation is
/// not trusted: a degenerate face (e.g. the coplanar seams a stacked face
/// extrude produces, ticket oa9h) falls back to an unordered loop, and chaining
/// it as-is would build a self-intersecting "bowtie" profile. An already
/// well-ordered loop assembles into the identical ring (every nearest-endpoint
/// distance is 0), so the ordered path loses nothing.
fn loop_rings(
    brep: &BRep,
    lp: &Loop,
    frame: &SketchFrame,
    placement: &Transform,
) -> Vec<Ring> {
    let mut chains: Vec<Vec<[f32; 2]>> = Vec::new();
    for &(edge_id, reversed) in &lp.edges {
        let mut pts = crate::brep::edge_sampling::sample_edge(&brep.edge(edge_id).geometry);
        if reversed {
            pts.reverse();
        }
        let uv: Vec<[f32; 2]> = pts
            .iter()
            .map(|p| project(place_point(placement, *p), frame))
            .collect();
        // Drop chains that project to a single point (an edge perpendicular
        // to the frame contributes no boundary and would stall the assembly).
        let degenerate = uv.len() < 2 || uv.iter().all(|p| dist2(*p, uv[0]) <= 1e-12);
        if !degenerate {
            chains.push(uv);
        }
    }
    // A coplanar seam is traced once per branch pair, so the loop can carry the
    // SAME boundary stretch several times — possibly split differently per copy
    // (ticket vyee). Duplicates make the nearest-endpoint assembly ambiguous and
    // strand orphans that can never close. Keep chains longest-first and drop
    // any chain that already lies ENTIRELY on the kept ones (coverage test, so
    // a split copy is absorbed by the longer original regardless of cut points).
    let mut lo = [f32::INFINITY; 2];
    let mut hi = [f32::NEG_INFINITY; 2];
    for c in &chains {
        for p in c {
            for k in 0..2 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
    }
    let diag = ((hi[0] - lo[0]).powi(2) + (hi[1] - lo[1]).powi(2)).sqrt();
    let tol = (1e-3 * diag).max(1e-5);
    chains.sort_by(|a, b| chain_len(b).total_cmp(&chain_len(a)));
    let mut unique: Vec<Vec<[f32; 2]>> = Vec::new();
    for c in chains {
        // Sample vertices AND segment midpoints: a 2-point chain (an exact
        // straight edge) shares both endpoints with its neighbours, so the
        // vertices alone would read as "covered" for every closing edge.
        let near = |p: [f32; 2]| unique.iter().any(|u| point_near_polyline(p, u, tol));
        let covered = c.iter().all(|p| near(*p))
            && c.windows(2).all(|w| {
                near([(w[0][0] + w[1][0]) * 0.5, (w[0][1] + w[1][1]) * 0.5])
            });
        if !covered {
            unique.push(c);
        }
    }
    assemble_rings(unique)
}

/// Length of a chain's polyline.
fn chain_len(c: &[[f32; 2]]) -> f32 {
    c.windows(2).map(|w| dist2(w[0], w[1]).sqrt()).sum()
}

/// Squared distance from point `p` to the segment `a→b`.
fn point_seg_dist2(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    let ab = [b[0] - a[0], b[1] - a[1]];
    let len2 = ab[0] * ab[0] + ab[1] * ab[1];
    let t = if len2 > 0.0 {
        (((p[0] - a[0]) * ab[0] + (p[1] - a[1]) * ab[1]) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    dist2(p, [a[0] + t * ab[0], a[1] + t * ab[1]])
}

/// `true` once `p` lies within `tol` of any segment of `chain`.
fn point_near_polyline(p: [f32; 2], chain: &[[f32; 2]], tol: f32) -> bool {
    let t2 = tol * tol;
    if chain.len() == 1 {
        return dist2(p, chain[0]) <= t2;
    }
    chain.windows(2).any(|w| point_seg_dist2(p, w[0], w[1]) <= t2)
}

/// Greedy nearest-endpoint ring assembly. Starting from any unused chain, keep
/// appending the unused chain whose nearer endpoint is closest to the current
/// ring end (reversing it when its far end matches), until CLOSING the ring
/// (gap back to the ring start) is at least as good as continuing — then start
/// the next ring. A self-closed chain (full circular rim) closes immediately.
fn assemble_rings(chains: Vec<Vec<[f32; 2]>>) -> Vec<Ring> {
    let n = chains.len();
    let mut used = vec![false; n];
    let mut rings: Vec<Ring> = Vec::new();
    for seed in 0..n {
        if used[seed] {
            continue;
        }
        used[seed] = true;
        let mut ring: Ring = vec![chains[seed].clone()];
        loop {
            let end = *ring.last().unwrap().last().unwrap();
            let start = ring[0][0];
            let close_d = dist2(end, start).sqrt();
            let mut best: Option<(usize, bool, f32)> = None;
            for (i, c) in chains.iter().enumerate() {
                if used[i] {
                    continue;
                }
                let df = dist2(end, c[0]).sqrt();
                let dl = dist2(end, *c.last().unwrap()).sqrt();
                let (rev, d) = if dl < df { (true, dl) } else { (false, df) };
                if best.is_none_or(|(_, _, bd)| d < bd) {
                    best = Some((i, rev, d));
                }
            }
            match best {
                Some((i, rev, d)) if d < close_d => {
                    used[i] = true;
                    let mut c = chains[i].clone();
                    if rev {
                        c.reverse();
                    }
                    ring.push(c);
                }
                _ => break,
            }
        }
        rings.push(ring);
    }
    rings
}

/// Distance from a ring's final point back to its first — the gap the implicit
/// closing segment has to bridge. ≈0 for a genuinely closed boundary.
fn ring_closing_gap(ring: &Ring) -> f32 {
    let start = ring[0][0];
    let end = *ring.last().unwrap().last().unwrap();
    dist2(end, start).sqrt()
}

/// Total polyline length of a ring's chains (excluding the closing segment).
fn ring_perimeter(ring: &Ring) -> f32 {
    let mut len = 0.0;
    for c in ring {
        for w in c.windows(2) {
            len += dist2(w[0], w[1]).sqrt();
        }
    }
    len
}

/// Mean of a ring's sample points — cheap positional fingerprint for the
/// duplicate test (NOT an area centroid; coincident rings have coincident
/// point sets, which is all the comparison needs).
fn ring_centroid(ring: &Ring) -> [f32; 2] {
    let mut c = [0.0f32; 2];
    let mut n = 0.0f32;
    for chain in ring {
        for p in chain {
            c[0] += p[0];
            c[1] += p[1];
            n += 1.0;
        }
    }
    [c[0] / n.max(1.0), c[1] / n.max(1.0)]
}

/// Same boundary traced twice? Area, extent and centroid must all agree within
/// a relative tolerance — duplicate seam traces differ only by the tracer's
/// activity band (≈1e-5 of the model), real distinct rings differ structurally.
fn rings_coincident(a: &Ring, b: &Ring) -> bool {
    let (da, db) = (ring_diag(a), ring_diag(b));
    let scale = da.max(db).max(1e-6);
    if (da - db).abs() > 1e-3 * scale {
        return false;
    }
    if (ring_area(a).abs() - ring_area(b).abs()).abs() > 1e-3 * scale * scale {
        return false;
    }
    let (ca, cb) = (ring_centroid(a), ring_centroid(b));
    dist2(ca, cb).sqrt() <= 1e-2 * scale
}

/// Diagonal of a ring's 2D AABB — its overall spatial extent.
fn ring_diag(ring: &Ring) -> f32 {
    let mut lo = [f32::INFINITY; 2];
    let mut hi = [f32::NEG_INFINITY; 2];
    for c in ring {
        for p in c {
            for k in 0..2 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
    }
    let d = [hi[0] - lo[0], hi[1] - lo[1]];
    (d[0] * d[0] + d[1] * d[1]).sqrt()
}

/// Shoelace area of a ring's concatenated sample points (sign carries the
/// winding; callers rank by |area| to pick the outer profile).
fn ring_area(ring: &Ring) -> f32 {
    let mut pts: Vec<[f32; 2]> = Vec::new();
    for c in ring {
        pts.extend_from_slice(c);
    }
    let mut area = 0.0;
    for i in 0..pts.len() {
        let j = (i + 1) % pts.len();
        area += pts[i][0] * pts[j][1] - pts[j][0] * pts[i][1];
    }
    area * 0.5
}

/// Classify each chain of an assembled ring into analytic 2D segments.
fn ring_to_segments(ring: &Ring) -> Vec<SketchSegment> {
    let mut segs: Vec<SketchSegment> = Vec::new();
    for uv in ring {
        segs.extend(edge_to_segments(uv));
    }
    segs
}

/// Body-local → world: `placement` applies scale, then rotation, then
/// translation (mirrors `face_plane_frame`'s centroid mapping).
fn place_point(placement: &Transform, p: [f64; 3]) -> [f32; 3] {
    let scaled = [
        p[0] as f32 * placement.scale[0],
        p[1] as f32 * placement.scale[1],
        p[2] as f32 * placement.scale[2],
    ];
    let r = crate::quat_rotate(placement.rotation, scaled);
    [
        r[0] + placement.translate[0],
        r[1] + placement.translate[1],
        r[2] + placement.translate[2],
    ]
}

/// World point → frame 2D: `(u, v) = ((p − origin)·x_axis, (p − origin)·y_axis)`.
/// The component along the normal is dropped (≈ 0 for an in-plane point).
fn project(p: [f32; 3], frame: &SketchFrame) -> [f32; 2] {
    let d = [p[0] - frame.origin[0], p[1] - frame.origin[1], p[2] - frame.origin[2]];
    [dot(d, frame.x_axis), dot(d, frame.y_axis)]
}

fn dist2(a: [f32; 2], b: [f32; 2]) -> f32 {
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    dx * dx + dy * dy
}

/// Classify one edge's projected point chain into analytic segments.
///
/// - collinear → a single exact `Line`,
/// - on a common circle → a single exact `Arc` (full `±2π` when the chain
///   closes, e.g. a cylinder cap rim),
/// - otherwise → a polyline of `Line`s (the general-spline fallback).
fn edge_to_segments(uv_in: &[[f32; 2]]) -> Vec<SketchSegment> {
    // Drop consecutive duplicates (Bézier-span C⁰ joins project onto the same
    // point); remember whether the *original* chain closed on itself.
    let mut uv: Vec<[f32; 2]> = Vec::with_capacity(uv_in.len());
    for &p in uv_in {
        if uv.last().is_none_or(|q| dist2(*q, p) > 1e-12) {
            uv.push(p);
        }
    }
    let n = uv.len();
    if n < 2 {
        return Vec::new();
    }
    if n == 2 {
        return vec![SketchSegment::Line { start: uv[0], end: uv[1] }];
    }

    // Tolerance scaled to the edge's own extent.
    let (mut mn, mut mx) = (uv[0], uv[0]);
    for p in &uv {
        mn = [mn[0].min(p[0]), mn[1].min(p[1])];
        mx = [mx[0].max(p[0]), mx[1].max(p[1])];
    }
    let diag = ((mx[0] - mn[0]).powi(2) + (mx[1] - mn[1]).powi(2)).sqrt();
    let tol = (diag * 1e-4).max(1e-6);

    // Collinear → one exact line spanning the whole edge.
    if max_perp_dist(&uv) <= tol {
        return vec![SketchSegment::Line { start: uv[0], end: uv[n - 1] }];
    }

    // Circular → one exact arc.
    let closed_loop = dist2(uv_in[0], uv_in[uv_in.len() - 1]).sqrt() <= tol;
    if let Some((center, radius)) = fit_circle(&uv, tol) {
        let start_angle = (uv[0][1] - center[1]).atan2(uv[0][0] - center[0]);
        let walked = total_sweep(&uv, center);
        let sweep_angle = if closed_loop {
            std::f32::consts::TAU * walked.signum()
        } else {
            walked
        };
        return vec![SketchSegment::Arc { center, radius, start_angle, sweep_angle }];
    }

    // General spline → polyline fallback (still exact distance per Line span).
    uv.windows(2)
        .map(|w| SketchSegment::Line { start: w[0], end: w[1] })
        .collect()
}

/// Largest perpendicular distance of any point from the chord `first→last`.
fn max_perp_dist(uv: &[[f32; 2]]) -> f32 {
    let a = uv[0];
    let b = uv[uv.len() - 1];
    let ab = [b[0] - a[0], b[1] - a[1]];
    let len = (ab[0] * ab[0] + ab[1] * ab[1]).sqrt();
    if len < 1e-9 {
        // Degenerate chord (closed edge): fall back to spread about `a`.
        return uv
            .iter()
            .map(|p| (dist2(*p, a)).sqrt())
            .fold(0.0, f32::max);
    }
    let (nx, ny) = (-ab[1] / len, ab[0] / len);
    uv.iter()
        .map(|p| ((p[0] - a[0]) * nx + (p[1] - a[1]) * ny).abs())
        .fold(0.0, f32::max)
}

/// Fit a circle through three well-spread points and accept it only if **every**
/// sample lies within `tol` of that circle. Returns `(center, radius)`.
fn fit_circle(uv: &[[f32; 2]], tol: f32) -> Option<([f32; 2], f32)> {
    let n = uv.len();
    // Three well-spread samples (≈120° apart on a circle). Avoid index `n-1`:
    // a closed rim's last sample coincides with the first, which would collapse
    // the circumcircle to two distinct points.
    let (a, b, c) = (uv[0], uv[n / 3], uv[(2 * n) / 3]);
    let (center, radius) = circumcircle(a, b, c)?;
    // Radius must be sane and every point on the circle within tolerance.
    let band = tol.max(radius * 1e-3);
    for p in uv {
        if ((dist2(*p, center)).sqrt() - radius).abs() > band {
            return None;
        }
    }
    Some((center, radius))
}

/// Circumcircle of three 2D points; `None` if (near-)collinear.
fn circumcircle(a: [f32; 2], b: [f32; 2], c: [f32; 2]) -> Option<([f32; 2], f32)> {
    let d = 2.0 * (a[0] * (b[1] - c[1]) + b[0] * (c[1] - a[1]) + c[0] * (a[1] - b[1]));
    if d.abs() < 1e-12 {
        return None;
    }
    let a2 = a[0] * a[0] + a[1] * a[1];
    let b2 = b[0] * b[0] + b[1] * b[1];
    let c2 = c[0] * c[0] + c[1] * c[1];
    let ux = (a2 * (b[1] - c[1]) + b2 * (c[1] - a[1]) + c2 * (a[1] - b[1])) / d;
    let uy = (a2 * (c[0] - b[0]) + b2 * (a[0] - c[0]) + c2 * (b[0] - a[0])) / d;
    let center = [ux, uy];
    Some((center, (dist2(center, a)).sqrt()))
}

/// Signed angle swept walking the chain around `center` (Σ wrapped deltas).
fn total_sweep(uv: &[[f32; 2]], center: [f32; 2]) -> f32 {
    let ang = |p: [f32; 2]| (p[1] - center[1]).atan2(p[0] - center[0]);
    let mut sum = 0.0_f32;
    for w in uv.windows(2) {
        let mut d = ang(w[1]) - ang(w[0]);
        while d > std::f32::consts::PI {
            d -= std::f32::consts::TAU;
        }
        while d < -std::f32::consts::PI {
            d += std::f32::consts::TAU;
        }
        sum += d;
    }
    sum
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn straight_chain_collapses_to_one_line() {
        let uv = [[0.0, 0.0], [1.0, 0.0], [2.0, 0.0], [3.0, 0.0]];
        let segs = edge_to_segments(&uv);
        assert_eq!(segs.len(), 1);
        match segs[0] {
            SketchSegment::Line { start, end } => {
                assert_eq!(start, [0.0, 0.0]);
                assert_eq!(end, [3.0, 0.0]);
            }
            other => panic!("expected Line, got {other:?}"),
        }
    }

    #[test]
    fn quarter_circle_becomes_one_arc() {
        // Unit circle, first quadrant sampled densely.
        let n = 33;
        let uv: Vec<[f32; 2]> = (0..=n)
            .map(|i| {
                let t = (i as f32 / n as f32) * std::f32::consts::FRAC_PI_2;
                [t.cos(), t.sin()]
            })
            .collect();
        let segs = edge_to_segments(&uv);
        assert_eq!(segs.len(), 1);
        match segs[0] {
            SketchSegment::Arc { center, radius, start_angle, sweep_angle } => {
                assert!((center[0]).abs() < 1e-3 && (center[1]).abs() < 1e-3, "center {center:?}");
                assert!((radius - 1.0).abs() < 1e-3, "radius {radius}");
                assert!(start_angle.abs() < 1e-3, "start {start_angle}");
                assert!(
                    (sweep_angle - std::f32::consts::FRAC_PI_2).abs() < 1e-2,
                    "sweep {sweep_angle}"
                );
            }
            other => panic!("expected Arc, got {other:?}"),
        }
    }

    #[test]
    fn full_circle_sweeps_tau() {
        let n = 64;
        let mut uv: Vec<[f32; 2]> = (0..=n)
            .map(|i| {
                let t = (i as f32 / n as f32) * std::f32::consts::TAU;
                [2.0 * t.cos(), 2.0 * t.sin()]
            })
            .collect();
        // Make the closing sample exactly coincide with the first.
        *uv.last_mut().unwrap() = uv[0];
        let segs = edge_to_segments(&uv);
        assert_eq!(segs.len(), 1);
        match segs[0] {
            SketchSegment::Arc { radius, sweep_angle, .. } => {
                assert!((radius - 2.0).abs() < 1e-3, "radius {radius}");
                assert!(
                    (sweep_angle.abs() - std::f32::consts::TAU).abs() < 1e-4,
                    "sweep {sweep_angle}"
                );
            }
            other => panic!("expected Arc, got {other:?}"),
        }
    }
}
