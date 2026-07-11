//! Hybrid-PASM P3/P4 — per-face Constrained Delaunay triangulation of faces
//! that admit a 2D chart (docs/HYBRID-PASM-MESHING-PLAN.md, ticket nwmi).
//!
//! P3: PLANAR faces, chart = the face plane (Newell frame of the outer loop).
//! P4: developable/periodic carriers (cylinder, cone, sphere bands) — the
//! caller maps everything into an unrolled `(u, v)` chart where `u` is the
//! scaled angle with period `P`; loops are unwrapped, classified by winding
//! number, and a full band (two loops of winding ±1) is cut open along a
//! seam whose LEFT and RIGHT copies carry the identical global vertex
//! sequence (shared Steiner points), so the seam welds structurally.
//!
//! Input per face: the boundary as CLOSED index chains (the B-Rep skeleton
//! points of the face's loops, in loop order) plus the free surface
//! particles attributed to that face. The loop segments become constraint
//! edges (`spade`), and only triangles whose centroid lies INSIDE the loops
//! (even-odd parity over all loops) are kept.
//!
//! Watertightness across faces is structural: both faces adjacent to a
//! model edge reference the SAME skeleton chain indices, so their boundary
//! triangulations share identical vertex sequences — no tolerance welding.

use spade::{ConstrainedDelaunayTriangulation, Point2, Triangulation};

/// One chart vertex: `(global output index, (u, v))`.
pub(crate) type UvPoint = (u32, [f64; 2]);

/// Core CDT over an arbitrary 2D chart. `loops` are closed boundary chains
/// (wrap implicit), `interior` are candidate free vertices (filtered to the
/// inside here). Back-mapping to global indices goes through the exact uv
/// bit patterns, so the SAME global index may appear at several uv positions
/// (periodic seam copies) — each position maps back to the one index.
///
/// `clearance`: interior candidates closer than this (chart units) to any
/// boundary segment are DROPPED. A particle exactly on a constraint would
/// make spade split it — in one face's chart but not its neighbour's — and
/// the shared-chain watertightness contract would break (the cylinder rim
/// found this: cap-plane particles sit at exactly v = cap height in the
/// barrel chart). The chains already sample the boundary, so nothing is lost.
fn triangulate_uv(
    loops: &[Vec<UvPoint>],
    interior: &[UvPoint],
    clearance: f64,
) -> Vec<[u32; 3]> {
    if loops.first().map_or(true, |l| l.len() < 3) {
        return Vec::new();
    }
    let mut cdt: ConstrainedDelaunayTriangulation<Point2<f64>> =
        ConstrainedDelaunayTriangulation::new();
    let keyf = |p: Point2<f64>| (p.x.to_bits(), p.y.to_bits());
    let mut back: std::collections::HashMap<(u64, u64), u32> = std::collections::HashMap::new();

    let loops2d: Vec<Vec<Point2<f64>>> = loops
        .iter()
        .map(|lp| lp.iter().map(|&(_, [u, v])| Point2::new(u, v)).collect())
        .collect();
    for (lp, lp2d) in loops.iter().zip(&loops2d) {
        for (&(gi, _), &p2) in lp.iter().zip(lp2d.iter()) {
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

    // Squared distance of a point to a loop segment.
    let seg_dist2 = |p: Point2<f64>, a: Point2<f64>, b: Point2<f64>| -> f64 {
        let (dx, dy) = (b.x - a.x, b.y - a.y);
        let len2 = dx * dx + dy * dy;
        let t = if len2 > 0.0 {
            (((p.x - a.x) * dx + (p.y - a.y) * dy) / len2).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let (ex, ey) = (p.x - (a.x + t * dx), p.y - (a.y + t * dy));
        ex * ex + ey * ey
    };
    let clearance2 = clearance * clearance;
    // Zusätzlich zur absoluten Clearance eine RELATIVE: näher als 10% der
    // jeweiligen SEGMENTLÄNGE darf kein freier Punkt an eine Constraint-Kante
    // (Ticket e6b21b96). Ein dichter Partikel-Teppich (Spacing ≪ h) neben
    // einem groben Rand-Segment erzeugte sonst Sliver-Dreiecke (Höhe ≈
    // Clearance, Basis ≈ Segmentlänge); umgekehrt lässt die relative Regel
    // dichte Punkte an dichten Ketten zu — beide Seiten skalieren mit.
    const REL_CLEARANCE: f64 = 0.1;
    let too_close = |p: Point2<f64>| -> bool {
        loops2d.iter().any(|lp2d| {
            (0..lp2d.len()).any(|k| {
                let a = lp2d[k];
                let b = lp2d[(k + 1) % lp2d.len()];
                let d2 = seg_dist2(p, a, b);
                let (dx, dy) = (b.x - a.x, b.y - a.y);
                let len2 = dx * dx + dy * dy;
                d2 < clearance2 || d2 < REL_CLEARANCE * REL_CLEARANCE * len2
            })
        })
    };

    for &(gi, [u, v]) in interior {
        let p2 = Point2::new(u, v);
        if !inside(p2) {
            continue; // particle belongs to another region / periodic copy
        }
        if too_close(p2) {
            continue; // would split (or crowd) a boundary constraint
        }
        if let Ok(h) = cdt.insert(p2) {
            let pos = cdt.vertex(h).position();
            back.entry(keyf(pos)).or_insert(gi);
        }
    }

    // ── Harvest: COMBINATORIAL even-odd classification ──
    // Flood-fill from the outer face, flipping parity at every constraint
    // edge. Unlike a centroid-in-polygon test this is exact: sliver
    // triangles hugging a nearly-collinear constraint chain (periodic seam
    // Steiner points) would misclassify geometrically, but combinatorially
    // every constraint edge gets an inside triangle on exactly one side —
    // which is precisely the manifold contract.
    let nf = cdt.num_all_faces();
    let mut adj: Vec<Vec<(usize, bool)>> = vec![Vec::new(); nf];
    for e in cdt.undirected_edges() {
        let c = cdt.is_constraint_edge(e.fix());
        let d = e.as_directed();
        let (f1, f2) = (d.face().fix().index(), d.rev().face().fix().index());
        adj[f1].push((f2, c));
        adj[f2].push((f1, c));
    }
    let mut state: Vec<Option<bool>> = vec![None; nf];
    let outer = cdt.outer_face().fix().index();
    state[outer] = Some(false);
    let mut queue = std::collections::VecDeque::from([outer]);
    while let Some(f) = queue.pop_front() {
        let cur = state[f].unwrap_or(false);
        for &(g, flip) in &adj[f] {
            if state[g].is_none() {
                state[g] = Some(cur ^ flip);
                queue.push_back(g);
            }
        }
    }

    let mut tris = Vec::new();
    for face in cdt.inner_faces() {
        if state[face.fix().index()] != Some(true) {
            continue;
        }
        let [a, b, c] = face.positions();
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
    clearance: f64,
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
    let to2d = |idx: u32| -> [f64; 2] {
        let p = points[idx as usize];
        let d = [
            p[0] as f64 - centroid[0],
            p[1] as f64 - centroid[1],
            p[2] as f64 - centroid[2],
        ];
        [
            d[0] * u[0] + d[1] * u[1] + d[2] * u[2],
            d[0] * v[0] + d[1] * v[1] + d[2] * v[2],
        ]
    };

    let uv_loops: Vec<Vec<UvPoint>> = loops
        .iter()
        .map(|lp| lp.iter().map(|&i| (i, to2d(i))).collect())
        .collect();
    let uv_interior: Vec<UvPoint> = interior.iter().map(|&i| (i, to2d(i))).collect();
    triangulate_uv(&uv_loops, &uv_interior, clearance)
}

/// Result of a periodic-chart face triangulation. `aux` are NEW seam Steiner
/// vertices the caller must append to the output point table; their global
/// indices start at the `aux_base` passed in.
pub(crate) struct PeriodicFaceMesh {
    pub tris: Vec<[u32; 3]>,
    pub aux: Vec<[f32; 3]>,
}

/// Unwrap a closed ring's `u` coordinates into a continuous chain (each step
/// shifted by the period multiple that minimises |Δu|) and return the ring's
/// winding number around the period.
fn unwrap_ring(ring: &[UvPoint], period: f64) -> (Vec<UvPoint>, i32) {
    let mut out: Vec<UvPoint> = Vec::with_capacity(ring.len());
    let mut prev = ring[0].1[0];
    for &(gi, [u, v]) in ring {
        let u2 = u - ((u - prev) / period).round() * period;
        out.push((gi, [u2, v]));
        prev = u2;
    }
    let back = ring[0].1[0] - ((ring[0].1[0] - prev) / period).round() * period;
    let w = ((back - out[0].1[0]) / period).round() as i32;
    (out, w)
}

/// Triangulate a face on a periodic chart (`u` has period `period`; both u
/// and v pre-scaled to comparable length units). Handles two shapes:
///
/// - **No winding loop** (a patch not going around): loops are branch-aligned
///   to the outer loop and triangulated directly.
/// - **A full band** (exactly two loops winding ±1, e.g. a cylinder barrel):
///   the band is cut open at the first vertex of the ascending loop. The cut
///   seam runs from that vertex to the nearest vertex of the other loop and
///   is sampled at `seam_h` into NEW Steiner vertices (world positions via
///   `world_of`). The seam's left and right copies carry the identical
///   global index sequence, so across the seam every edge is shared by
///   exactly two triangles — the cut is invisible in the mesh.
///
/// - **A pole cap** (exactly one loop winding once, `poles` non-empty): an
///   artificial POLE RING of aux vertices is placed just short of the
///   enclosed pole (chosen from `poles` — chart v values of the carrier's
///   poles/apexes — via the interior particles' side, nearest as fallback).
///   The loop↔ring strip recurses through the band path; the ring is then
///   closed by an explicit triangle FAN to one pole vertex, so every fan
///   triangle touches the pole exactly once and nothing collapses.
///
/// Anything else (|w| ≥ 2, holes crossing the seam, cap without a known
/// pole) returns `None` — the face stays unmeshed rather than risking a
/// broken seam.
pub(crate) fn triangulate_periodic_face(
    loops_uv: &[Vec<UvPoint>],
    interior: &[UvPoint],
    period: f64,
    seam_h: f64,
    clearance: f64,
    poles: &[f64],
    aux_base: u32,
    world_of: &dyn Fn(f64, f64) -> [f32; 3],
) -> Option<PeriodicFaceMesh> {
    if !(period > 0.0) || loops_uv.iter().any(|l| l.len() < 3) || loops_uv.is_empty() {
        return None;
    }
    let unwrapped: Vec<(Vec<UvPoint>, i32)> =
        loops_uv.iter().map(|l| unwrap_ring(l, period)).collect();
    let winders: Vec<usize> = (0..unwrapped.len())
        .filter(|&i| unwrapped[i].1 != 0)
        .collect();

    // ── Case 1: no loop winds — align branches and triangulate directly ──
    if winders.is_empty() {
        let ref_mean = loop_mean_u(&unwrapped[0].0);
        let aligned: Vec<Vec<UvPoint>> = unwrapped
            .iter()
            .map(|(lp, _)| shift_loop(lp, ((ref_mean - loop_mean_u(lp)) / period).round() * period))
            .collect();
        let aligned_interior: Vec<UvPoint> = interior
            .iter()
            .map(|&(gi, [u, v])| {
                (gi, [u - ((u - ref_mean) / period).round() * period, v])
            })
            .collect();
        return Some(PeriodicFaceMesh {
            tris: triangulate_uv(&aligned, &aligned_interior, clearance),
            aux: Vec::new(),
        });
    }

    // ── Case 2: a pole cap — one loop winding once, pole known ──
    if winders.len() == 1 && unwrapped[winders[0]].1.abs() == 1 && !poles.is_empty() {
        let wa = winders[0];
        let loop_vs: Vec<f64> = unwrapped[wa].0.iter().map(|&(_, [_, v])| v).collect();
        let mean_loop_v = loop_vs.iter().sum::<f64>() / loop_vs.len() as f64;
        // Pick the enclosed pole: the interior particles lie between loop
        // and pole, so their mean v points the way; nearest pole otherwise.
        let dist = |p: f64| (p - mean_loop_v).abs();
        let mut pole_v = poles[0];
        for &p in poles {
            if dist(p) < dist(pole_v) {
                pole_v = p;
            }
        }
        if !interior.is_empty() {
            let mean_int_v =
                interior.iter().map(|&(_, [_, v])| v).sum::<f64>() / interior.len() as f64;
            let side = mean_int_v - mean_loop_v;
            let mut best: Option<f64> = None;
            for &p in poles {
                if (p - mean_loop_v) * side > 0.0 && best.map_or(true, |b| dist(p) < dist(b)) {
                    best = Some(p);
                }
            }
            if let Some(p) = best {
                pole_v = p;
            }
        }
        let min_gap = loop_vs
            .iter()
            .map(|v| (v - pole_v).abs())
            .fold(f64::INFINITY, f64::min);
        if !(min_gap > 1e-9) {
            return None; // loop touches the pole — degenerate cap
        }
        let dir = (mean_loop_v - pole_v).signum();
        let ring_v = pole_v + dir * seam_h.min(0.5 * min_gap).max(1e-9);

        // Ring resolution from WORLD arc length — the chart's u scale is a
        // reference radius, but near the pole the true circumference shrinks.
        let wdist = |a: [f32; 3], b: [f32; 3]| -> f64 {
            let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
            ((d[0] * d[0] + d[1] * d[1] + d[2] * d[2]) as f64).sqrt()
        };
        let a0u_cap = unwrapped[wa].0[0].1[0];
        let probes = 24usize;
        let mut ring_len = 0.0f64;
        let mut prev = world_of(a0u_cap, ring_v);
        for i in 1..=probes {
            let q = world_of(a0u_cap + i as f64 / probes as f64 * period, ring_v);
            ring_len += wdist(q, prev);
            prev = q;
        }
        let h_world = wdist(
            world_of(a0u_cap, ring_v),
            world_of(a0u_cap, ring_v + dir * seam_h),
        )
        .max(1e-9);
        let n_ring = ((ring_len / h_world).round() as usize).clamp(3, 512);

        // Pole ring + pole vertex as NEW aux points, then the loop↔ring
        // strip through the band path (recursion with poles = [] — the
        // strip has two winding loops now, so it cannot recurse again).
        let mut aux: Vec<[f32; 3]> = Vec::with_capacity(n_ring + 1);
        let mut ring_loop: Vec<UvPoint> = Vec::with_capacity(n_ring);
        for i in 0..n_ring {
            let u = a0u_cap + i as f64 / n_ring as f64 * period;
            ring_loop.push((aux_base + i as u32, [u, ring_v]));
            aux.push(world_of(u, ring_v));
        }
        let pole_idx = aux_base + n_ring as u32;
        aux.push(world_of(a0u_cap, pole_v));

        let mut sub_loops = loops_uv.to_vec();
        sub_loops.push(ring_loop);
        let sub = triangulate_periodic_face(
            &sub_loops,
            interior,
            period,
            seam_h,
            clearance,
            &[],
            pole_idx + 1,
            world_of,
        )?;
        let mut tris = sub.tris;
        for i in 0..n_ring {
            tris.push([
                pole_idx,
                aux_base + i as u32,
                aux_base + ((i + 1) % n_ring) as u32,
            ]);
        }
        aux.extend(sub.aux);
        return Some(PeriodicFaceMesh { tris, aux });
    }

    // ── Case 3: a full band — exactly two loops winding once each ──
    if winders.len() != 2 || winders.iter().any(|&i| unwrapped[i].1.abs() != 1) {
        return None; // exotic topology (|w| ≥ 2, cap without pole) — unmeshed
    }
    let (wa, wb) = (winders[0], winders[1]);

    // Loop A ascending in u.
    let mut a_raw = loops_uv[wa].clone();
    if unwrapped[wa].1 < 0 {
        a_raw.reverse();
    }
    let (a_chain, _) = unwrap_ring(&a_raw, period);
    let (a0u, a0v) = (a_chain[0].1[0], a_chain[0].1[1]);

    // Loop B descending, rotated to start at the vertex nearest the cut.
    let mut b_raw = loops_uv[wb].clone();
    if unwrapped[wb].1 > 0 {
        b_raw.reverse();
    }
    let (mut r, mut best, mut dr) = (0usize, f64::INFINITY, 0.0f64);
    for (j, &(_, [u, _])) in b_raw.iter().enumerate() {
        let mut d = u - a0u;
        d -= (d / period).round() * period;
        if d.abs() < best {
            (r, best, dr) = (j, d.abs(), d);
        }
    }
    let rot: Vec<UvPoint> = b_raw[r..].iter().chain(&b_raw[..r]).copied().collect();
    let (mut b_chain, _) = unwrap_ring(&rot, period);
    // Place B's start at exactly one period right of the cut (+ the residual
    // offset dr), so the seam between a_0 and B's start is as short as the
    // chart allows.
    let target = a0u + period + dr;
    let shift = target - b_chain[0].1[0];
    for p in &mut b_chain {
        p.1[0] += shift;
    }
    let vb = b_chain[0].1[1];

    // Seam Steiner points between a_0 = (a0u, a0v) and B-low = (a0u+dr, vb),
    // shared by both copies (same aux index at u and u+period).
    let seam_len = (dr * dr + (vb - a0v) * (vb - a0v)).sqrt();
    let segments = if seam_h > 0.0 {
        (seam_len / seam_h).round().max(1.0) as usize
    } else {
        1
    };
    let mut aux: Vec<[f32; 3]> = Vec::with_capacity(segments.saturating_sub(1));
    let mut seam: Vec<UvPoint> = Vec::new(); // interior Steiner pts, a→b order
    for i in 1..segments {
        let t = i as f64 / segments as f64;
        let (su, sv) = (a0u + t * dr, a0v + t * (vb - a0v));
        seam.push((aux_base + aux.len() as u32, [su, sv]));
        aux.push(world_of(su, sv));
    }

    // Outer polygon: A ascending, right seam up, B descending, left seam back.
    let mut poly: Vec<UvPoint> = Vec::new();
    poly.extend(a_chain.iter().copied());
    poly.push((a_chain[0].0, [a0u + period, a0v])); // a_0 at the +P copy
    for &(gi, [u, v]) in &seam {
        poly.push((gi, [u + period, v])); // right seam copy
    }
    poly.extend(b_chain.iter().copied()); // starts at B-high = B-low + P
    poly.push((b_chain[0].0, [a0u + dr, vb])); // B-low (same global index)
    for &(gi, [u, v]) in seam.iter().rev() {
        poly.push((gi, [u, v])); // left seam copy — identical indices
    }

    // Hole loops (non-winding) must fit strictly between the two seam
    // copies; align each to the band centre first.
    let centre = a0u + 0.5 * period + 0.5 * dr;
    let (safe_lo, safe_hi) = (a0u + dr.max(0.0), a0u + period + dr.min(0.0));
    let mut all_loops: Vec<Vec<UvPoint>> = vec![poly];
    for (i, (lp, _)) in unwrapped.iter().enumerate() {
        if i == wa || i == wb {
            continue;
        }
        let shifted = shift_loop(lp, ((centre - loop_mean_u(lp)) / period).round() * period);
        let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
        for &(_, [u, _]) in &shifted {
            lo = lo.min(u);
            hi = hi.max(u);
        }
        if lo <= safe_lo || hi >= safe_hi {
            return None; // hole crosses the seam — give up on this face
        }
        all_loops.push(shifted);
    }

    // Interior particles: submit both periodic copies covering the band's
    // u-span; the inside() parity picks the one that falls into the polygon.
    let base_lo = a0u + dr.min(0.0);
    let mut candidates: Vec<UvPoint> = Vec::with_capacity(interior.len() * 2);
    for &(gi, [u, v]) in interior {
        let u0 = u - ((u - base_lo) / period).floor() * period;
        candidates.push((gi, [u0, v]));
        candidates.push((gi, [u0 + period, v]));
    }

    Some(PeriodicFaceMesh {
        tris: triangulate_uv(&all_loops, &candidates, clearance),
        aux,
    })
}

fn loop_mean_u(lp: &[UvPoint]) -> f64 {
    lp.iter().map(|&(_, [u, _])| u).sum::<f64>() / lp.len() as f64
}

fn shift_loop(lp: &[UvPoint], du: f64) -> Vec<UvPoint> {
    lp.iter().map(|&(gi, [u, v])| (gi, [u + du, v])).collect()
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
            0.1,
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
        let tris = triangulate_planar_face(&pts, &[vec![0, 1, 2]], &[], 0.1);
        assert_eq!(tris.len(), 1);
    }

    /// Hybrid-PASM P4: a full cylinder band (two rings winding once each)
    /// is cut open along a seam whose two copies share global indices — the
    /// resulting strip is CLOSED around the period: its only boundary edges
    /// are the two rings; every other edge is shared by exactly two
    /// triangles, and the seam is invisible.
    #[test]
    fn periodic_band_welds_its_seam() {
        let period = 80.0f64;
        let n = 8usize;
        // Bottom ring 0..8 at v=0, top ring 8..16 at v=10 (raw u in [0, P)).
        let ring = |base: u32, v: f64, off: f64| -> Vec<UvPoint> {
            (0..n)
                .map(|i| (base + i as u32, [off + i as f64 * period / n as f64, v]))
                .collect()
        };
        let bottom = ring(0, 0.0, 0.0);
        // The top ring winds the OTHER way (as a band boundary would) and
        // starts phase-shifted so the seam is genuinely slanted.
        let mut top = ring(8, 10.0, 3.0);
        top.reverse();
        // One free particle inside the band.
        let interior = vec![(16u32, [40.0f64, 5.0])];
        let world_of = |u: f64, v: f64| -> [f32; 3] {
            let r = period / std::f64::consts::TAU;
            let th = u / r;
            [(r * th.cos()) as f32, v as f32, (r * th.sin()) as f32]
        };
        let mesh = triangulate_periodic_face(
            &[bottom, top],
            &interior,
            period,
            4.0,
            1.0,
            &[],
            17,
            &world_of,
        )
        .expect("band must triangulate");
        assert!(!mesh.tris.is_empty());
        assert!(!mesh.aux.is_empty(), "seam should get Steiner points");

        let mut edge_count: std::collections::HashMap<(u32, u32), u32> =
            std::collections::HashMap::new();
        let mut used: std::collections::HashSet<u32> = std::collections::HashSet::new();
        for t in &mesh.tris {
            assert!(t[0] != t[1] && t[1] != t[2] && t[2] != t[0]);
            for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                *edge_count.entry((a.min(b), a.max(b))).or_default() += 1;
                used.insert(a);
                used.insert(b);
            }
        }
        // All 16 ring vertices and the interior particle are used.
        for gi in 0..17u32 {
            assert!(used.contains(&gi), "vertex {gi} unused");
        }
        // Boundary edges (count 1) may ONLY be ring segments — consecutive
        // indices within the same ring (mod ring size). Everything else,
        // including every seam edge, must be shared by exactly two.
        for (&(a, b), &c) in &edge_count {
            assert!(c <= 2, "edge {a}-{b} used {c}× (non-manifold)");
            if c == 1 {
                let same_ring = (a < 8 && b < 8) || ((8..16).contains(&a) && (8..16).contains(&b));
                assert!(
                    same_ring && ((a as i32 - b as i32).abs() == 1 || (a as i32 - b as i32).abs() == 7),
                    "open edge {a}-{b} is not a ring segment — the seam leaked"
                );
            }
        }
    }

    /// A single winding loop WITHOUT a known pole must be refused — meshing
    /// it through the chart would collapse the pole line.
    #[test]
    fn pole_cap_without_pole_is_refused() {
        let period = 80.0f64;
        let lp: Vec<UvPoint> = (0..8)
            .map(|i| (i as u32, [i as f64 * 10.0, 5.0]))
            .collect();
        let world_of = |_: f64, _: f64| [0.0f32; 3];
        assert!(
            triangulate_periodic_face(&[lp], &[], period, 4.0, 1.0, &[], 8, &world_of).is_none()
        );
    }

    /// Hybrid-PASM P4c: a cap (one winding loop) with a known pole closes
    /// via pole ring + fan — the only boundary edges of the result are the
    /// original loop's segments; the pole vertex is used, and every fan
    /// triangle touches it exactly once.
    #[test]
    fn pole_cap_fans_to_the_pole() {
        let period = 80.0f64;
        let su = period / std::f64::consts::TAU;
        // Cone-like chart: world radius grows linearly with v, apex at v=0.
        let world_of = move |u: f64, v: f64| -> [f32; 3] {
            let th = u / su;
            let r = v.max(0.0);
            [(r * th.cos()) as f32, v as f32, (r * th.sin()) as f32]
        };
        let lp: Vec<UvPoint> = (0..8)
            .map(|i| (i as u32, [3.0 + i as f64 * 10.0, 5.0]))
            .collect();
        let mesh = triangulate_periodic_face(
            &[lp],
            &[],
            period,
            2.0,
            0.5,
            &[0.0],
            8,
            &world_of,
        )
        .expect("cap must triangulate");
        assert!(!mesh.aux.is_empty(), "ring + pole aux vertices expected");
        let mut edge_count: std::collections::HashMap<(u32, u32), u32> =
            std::collections::HashMap::new();
        let mut used: std::collections::HashSet<u32> = std::collections::HashSet::new();
        for t in &mesh.tris {
            assert!(t[0] != t[1] && t[1] != t[2] && t[2] != t[0]);
            for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                *edge_count.entry((a.min(b), a.max(b))).or_default() += 1;
                used.insert(a);
                used.insert(b);
            }
        }
        // Boundary edges (count 1) may only be original loop segments
        // (indices 0..8, cyclically adjacent); everything else, including
        // ring and fan edges, must be shared by exactly two triangles.
        for (&(a, b), &c) in &edge_count {
            assert!(c <= 2, "edge {a}-{b} used {c}× (non-manifold)");
            if c == 1 {
                assert!(
                    a < 8
                        && b < 8
                        && ((a as i32 - b as i32).abs() == 1
                            || (a as i32 - b as i32).abs() == 7),
                    "open edge {a}-{b} is not a rim segment — cap leaked"
                );
            }
        }
        // Euler characteristic of a disc: V − E + F = 1.
        let v = used.len() as i64;
        let e = edge_count.len() as i64;
        let f = mesh.tris.len() as i64;
        assert_eq!(
            v - e + f,
            1,
            "cap Euler characteristic broke: V={v} E={e} F={f}"
        );
    }
}
