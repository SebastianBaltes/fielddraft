//! Hybrid-PASM P3 — per-face Constrained Delaunay triangulation of PLANAR
//! faces (docs/HYBRID-PASM-MESHING-PLAN.md, ticket nwmi).
//!
//! Input per face: the boundary as CLOSED index chains (the B-Rep skeleton
//! points of the face's outer + inner loops, in loop order) plus the free
//! surface particles attributed to that face. Everything is projected into
//! the face plane (Newell frame from the outer loop), triangulated with the
//! loop segments as constraint edges (`spade`), and only triangles whose
//! centroid lies INSIDE the loops (even-odd parity over all loops — outer
//! hit once = in, hole hit too = out) are kept.
//!
//! Watertightness across faces is structural: both faces adjacent to a
//! model edge reference the SAME skeleton chain indices, so their boundary
//! triangulations share identical vertex sequences — no tolerance welding.

use spade::{ConstrainedDelaunayTriangulation, Point2, Triangulation};

/// Triangulate one planar face. `points` is the global output point table;
/// `loops` are closed boundary chains (first == last NOT repeated — the chain
/// wraps implicitly); `interior` are candidate free particles (filtered to
/// the inside here, so callers may pass every particle of the branch).
/// Returns triangles as global indices, or an empty list when the face is
/// degenerate (loops too short, zero-area frame, spade insertion failure).
pub(crate) fn triangulate_planar_face(
    points: &[[f32; 3]],
    loops: &[Vec<u32>],
    interior: &[u32],
) -> Vec<[u32; 3]> {
    let Some(outer) = loops.first() else {
        return Vec::new();
    };
    if outer.len() < 3 {
        return Vec::new();
    }

    // ── Newell frame of the outer loop (robust for any planar polygon) ──
    let mut normal = [0.0f64; 3];
    let mut centroid = [0.0f64; 3];
    for k in 0..outer.len() {
        let a = points[outer[k] as usize];
        let b = points[outer[(k + 1) % outer.len()] as usize];
        normal[0] += (a[1] as f64 - b[1] as f64) * (a[2] as f64 + b[2] as f64);
        normal[1] += (a[2] as f64 - b[2] as f64) * (a[0] as f64 + b[0] as f64);
        normal[2] += (a[0] as f64 - b[0] as f64) * (a[1] as f64 + b[1] as f64);
        centroid[0] += a[0] as f64;
        centroid[1] += a[1] as f64;
        centroid[2] += a[2] as f64;
    }
    let nl = (normal[0] * normal[0] + normal[1] * normal[1] + normal[2] * normal[2]).sqrt();
    if nl < 1e-12 {
        return Vec::new();
    }
    let n = [normal[0] / nl, normal[1] / nl, normal[2] / nl];
    let inv = 1.0 / outer.len() as f64;
    centroid = [centroid[0] * inv, centroid[1] * inv, centroid[2] * inv];
    let pick = if n[0].abs() < 0.9 { [1.0, 0.0, 0.0] } else { [0.0, 1.0, 0.0] };
    let u = {
        let c = [
            n[1] * pick[2] - n[2] * pick[1],
            n[2] * pick[0] - n[0] * pick[2],
            n[0] * pick[1] - n[1] * pick[0],
        ];
        let l = (c[0] * c[0] + c[1] * c[1] + c[2] * c[2]).sqrt();
        [c[0] / l, c[1] / l, c[2] / l]
    };
    let v = [
        n[1] * u[2] - n[2] * u[1],
        n[2] * u[0] - n[0] * u[2],
        n[0] * u[1] - n[1] * u[0],
    ];
    let to2d = |idx: u32| -> Point2<f64> {
        let p = points[idx as usize];
        let d = [
            p[0] as f64 - centroid[0],
            p[1] as f64 - centroid[1],
            p[2] as f64 - centroid[2],
        ];
        Point2::new(
            d[0] * u[0] + d[1] * u[1] + d[2] * u[2],
            d[0] * v[0] + d[1] * v[1] + d[2] * v[2],
        )
    };

    // ── CDT: loop chains as constraints, particles as free vertices ──
    // spade handles are mapped back to GLOBAL indices via the exact 2D
    // coordinates (both sides project from the same f32 source, so the bit
    // patterns match; duplicates collapse to the first index, which keeps the
    // seam contract intact).
    let mut cdt: ConstrainedDelaunayTriangulation<Point2<f64>> =
        ConstrainedDelaunayTriangulation::new();
    let keyf = |p: Point2<f64>| (p.x.to_bits(), p.y.to_bits());
    let mut back: std::collections::HashMap<(u64, u64), u32> = std::collections::HashMap::new();

    let loops2d: Vec<Vec<Point2<f64>>> = loops
        .iter()
        .map(|lp| lp.iter().map(|&i| to2d(i)).collect())
        .collect();
    for (lp, lp2d) in loops.iter().zip(&loops2d) {
        for (&gi, &p2) in lp.iter().zip(lp2d.iter()) {
            back.entry(keyf(p2)).or_insert(gi);
        }
        for k in 0..lp2d.len() {
            let a = lp2d[k];
            let b = lp2d[(k + 1) % lp2d.len()];
            if (a.x - b.x).abs() < 1e-12 && (a.y - b.y).abs() < 1e-12 {
                continue; // duplicate chain junction — no zero-length constraint
            }
            // A failed insertion (degenerate coordinates) degrades this face
            // to "unmeshed" rather than poisoning the whole export.
            if cdt.add_constraint_edge(a, b).is_err() {
                return Vec::new();
            }
        }
    }

    // Even-odd point-in-loops test in 2D (parity over ALL loops: inside the
    // outer ring = 1 crossing, additionally inside a hole = 2 → out).
    let inside = |p: Point2<f64>| -> bool {
        let mut crossings = 0u32;
        for lp2d in &loops2d {
            for k in 0..lp2d.len() {
                let a = lp2d[k];
                let b = lp2d[(k + 1) % lp2d.len()];
                if (a.y > p.y) != (b.y > p.y) {
                    let t = (p.y - a.y) / (b.y - a.y);
                    if a.x + t * (b.x - a.x) > p.x {
                        crossings += 1;
                    }
                }
            }
        }
        crossings % 2 == 1
    };

    for &gi in interior {
        let p2 = to2d(gi);
        if !inside(p2) {
            continue; // particle belongs to another region of the same branch
        }
        if let Ok(h) = cdt.insert(p2) {
            let pos = cdt.vertex(h).position();
            back.entry(keyf(pos)).or_insert(gi);
        }
    }

    // ── Harvest: keep faces whose centroid lies inside the loops ──
    let mut tris = Vec::new();
    for face in cdt.inner_faces() {
        let [a, b, c] = face.positions();
        let cx = Point2::new((a.x + b.x + c.x) / 3.0, (a.y + b.y + c.y) / 3.0);
        if !inside(cx) {
            continue;
        }
        let (Some(&ia), Some(&ib), Some(&ic)) = (
            back.get(&keyf(a)),
            back.get(&keyf(b)),
            back.get(&keyf(c)),
        ) else {
            continue; // vertex spade split in (constraint intersection) — skip
        };
        if ia == ib || ib == ic || ic == ia {
            continue;
        }
        tris.push([ia, ib, ic]);
    }
    tris
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn square_with_hole_and_interior_points() {
        // Unit-ish square (side 4) with a centred square hole (side 2), plus
        // interior particles: one in the ring (kept), one inside the hole
        // (must be excluded).
        let mut pts: Vec<[f32; 3]> = Vec::new();
        let mut outer = Vec::new();
        for (x, y) in [(-2.0, -2.0), (2.0, -2.0), (2.0, 2.0), (-2.0, 2.0)] {
            outer.push(pts.len() as u32);
            pts.push([x, y, 0.0]);
        }
        let mut hole = Vec::new();
        for (x, y) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
            hole.push(pts.len() as u32);
            pts.push([x, y, 0.0]);
        }
        let ring_particle = pts.len() as u32;
        pts.push([1.5, 0.0, 0.0]);
        let hole_particle = pts.len() as u32;
        pts.push([0.0, 0.0, 0.0]);

        let tris = triangulate_planar_face(
            &pts,
            &[outer, hole],
            &[ring_particle, hole_particle],
        );
        assert!(!tris.is_empty(), "ring must triangulate");
        // No triangle references the hole-interior particle, and no triangle
        // centroid lies inside the hole.
        for t in &tris {
            assert!(
                !t.contains(&hole_particle),
                "hole-interior particle leaked into the mesh"
            );
            let c = [
                (pts[t[0] as usize][0] + pts[t[1] as usize][0] + pts[t[2] as usize][0]) / 3.0,
                (pts[t[0] as usize][1] + pts[t[1] as usize][1] + pts[t[2] as usize][1]) / 3.0,
            ];
            assert!(
                !(c[0].abs() < 1.0 && c[1].abs() < 1.0),
                "triangle centroid inside the hole: {c:?}"
            );
        }
        // The ring particle IS referenced (it subdivides the ring).
        assert!(
            tris.iter().any(|t| t.contains(&ring_particle)),
            "ring particle unused"
        );
        // 2-manifold interior: every non-boundary edge shared by exactly 2.
        let mut edge_count: std::collections::HashMap<(u32, u32), u32> =
            std::collections::HashMap::new();
        for t in &tris {
            for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                *edge_count.entry((a.min(b), a.max(b))).or_default() += 1;
            }
        }
        for (&(a, b), &n) in &edge_count {
            assert!(n <= 2, "edge {a}-{b} used {n} times (non-manifold)");
        }
    }

    #[test]
    fn tilted_plane_triangulates_in_its_own_frame() {
        // A triangle loop on an oblique plane — the Newell frame must handle
        // arbitrary orientation.
        let pts = vec![
            [0.0f32, 0.0, 0.0],
            [2.0, 0.0, 1.0],
            [0.0, 2.0, 1.0],
        ];
        let tris = triangulate_planar_face(&pts, &[vec![0, 1, 2]], &[]);
        assert_eq!(tris.len(), 1);
    }
}
