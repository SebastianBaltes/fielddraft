//! B-Rep extraction, **Phase 3.5: topology reconstruction (edge walker)**.
//!
//! Implements the edge walker described in
//! `docs/GEOMETRY-KERNEL-CONCEPT.md` ("Phase 3.5: Topologie-Rekonstruktion
//! (Edge Walker)"). Phase 3 ([`crate::brep::extraction`]) hands us an
//! *unordered* `Vec<EdgePoint>` (one per `Edge` cell) plus one
//! [`crate::brep::Vertex`] per `Vertex` cell. This phase turns that point
//! cloud into **ordered edge chains**, each a `Vec<HermiteSample>` running
//! from one vertex to another (or a closed loop with no vertices), and
//! assembles them into a **wireframe** [`BRep`] — deduplicated vertices and
//! Hermite edges bounded by `start`/`end` [`VertexId`]s. Faces / shells /
//! solids stay empty; that is C.6.
//!
//! ## Algorithm (faithful to the concept, robustified)
//!
//! 1. **Vertex dedup (DL-13).** Phase 3 emits one `Vertex` per vertex *cell*;
//!    many cells share the same branch-triple. We deduplicate by the canonical
//!    branch-set (`Vertex::branches`, already sorted+deduped) and take the
//!    centroid of the merged cells' positions. A clean cube collapses to
//!    exactly 8 vertices.
//! 2. **Group edge points by signature** `(BranchId, BranchId)`.
//! 3. **Per-group global primitive fit (DL-51).** A signature group from a
//!    sketch-swept body (extrude / revolve) fragments into many gappy spatial
//!    runs even though it is *one* topological edge, because the sketch-SDF
//!    edge classification dithers contributors along the feature. We therefore
//!    first try to fit the **whole** group with a single straight line
//!    (total-least-squares) or a single circle (PCA plane + Kåsa fit); either
//!    collapses the fragments into one clean oriented chain. Only when neither
//!    primitive fits (a genuinely curved or multi-edge signature) do we fall
//!    back to **greedy spatial chaining (DL-11):** a nearest-neighbour walk
//!    that **splits into multiple chains** when the gap exceeds
//!    [`CHAIN_GAP_FACTOR`] × the median local spacing (DL-12), so two disjoint
//!    sub-edges sharing one signature do not get welded together.
//! 4. **Bound chains by vertices (DL-14).** For a chain with signature
//!    `(a, b)` the candidate end-vertices are the deduped vertices whose
//!    branch-set ⊇ `{a, b}`. Each chain endpoint snaps to the nearest such
//!    candidate within [`SNAP_FACTOR`] × the chain's local spacing. Chains
//!    with no vertex candidates whose two ends are spatially close are treated
//!    as **closed loops** (`start = end = None`, DL-15).
//! 5. **Assemble the wireframe.** Add the deduped vertices first (recording
//!    their `VertexId` keyed by branch-set), then one Hermite [`Edge`] per
//!    chain with the resolved start/end.
//!
//! Tangents are kept **monotone** along each chain: a sample's tangent is
//! flipped if it points against the local walk direction, so the resulting
//! Hermite chain is consistently oriented start→end.

use std::collections::HashMap;

use nalgebra::{Matrix3, SymmetricEigen, Vector3};

use crate::brep::extraction::{extract_features, extract_vertex, EdgePoint};
use crate::brep::octree::{CellClass, Octree, OctreeCell};
use crate::brep::{BRep, Edge, EdgeGeometry, EdgeId, HermiteSample, Vertex, VertexId};
use crate::bvh::BoundedCsg;
use crate::csg::{Aabb, BranchId};

/// A nearest-neighbour gap larger than `CHAIN_GAP_FACTOR ×` the median local
/// spacing of the group ends the current chain and starts a new one. Three is
/// a generous bound: along a single edge consecutive samples sit roughly one
/// cell apart (spacing ≈ const), so a real gap to a *disjoint* sub-edge — at
/// least the body's feature size away — is many medians wide, while ordinary
/// jitter stays well under 3×. See DL-12.
const CHAIN_GAP_FACTOR: f64 = 3.0;

/// A chain endpoint snaps to a candidate vertex when it lies within
/// `SNAP_FACTOR ×` the chain's local spacing of that vertex. The last edge
/// sample sits about one octree cell short of the true corner (the corner is a
/// `Vertex` cell, not an `Edge` cell), so the gap to snap across is ~one
/// spacing; `SNAP_FACTOR = 2.5` covers that plus the half-cell the resolved
/// vertex centroid may be offset, without reaching a different corner of the
/// same edge (a whole edge length away). See DL-14.
const SNAP_FACTOR: f64 = 2.5;

/// A vertex-less chain is a closed loop when the distance between its two ends
/// is at most `LOOP_FACTOR ×` the chain's local spacing — i.e. the ends meet
/// within roughly one sample step. See DL-15.
const LOOP_FACTOR: f64 = 2.5;

/// A signature group's deduped points are accepted as a single straight line
/// or single circle when the **max** fit residual is within
/// `FIT_RESIDUAL_FACTOR ×` the group's local spacing. The sketch-SDF edge
/// classification dithers points by up to roughly half a cell off the true
/// analytic feature, so the bound is a small multiple of one sample step;
/// loose enough to absorb that jitter, tight enough that a genuinely curved /
/// multi-edge group (whose points cannot lie on one line or circle) fails the
/// fit and falls back to gap-chaining. See DL-51.
const FIT_RESIDUAL_FACTOR: f64 = 1.5;

/// Minimum fraction of a group's points that must lie on the dominant fitted
/// plane (within `FIT_RESIDUAL_FACTOR × spacing`) for a robust circle fit to be
/// accepted. A subtracted curved primitive (a drilled cylinder) contaminates
/// the trim-circle signature group with cylinder-*wall* samples one or two
/// cells deep along the bore axis — points that are on the surface and carry
/// the trim signature (their octree cell's corner union spans the seam), but
/// sit at a different axial offset, so the raw group is *not* coplanar. The
/// true trim ring is the dominant coplanar cluster; we fit the circle to that
/// inlier plane and discard the axial tail, but only when the inliers are a
/// substantial share (a real circle, not a fluke sub-cluster). 0.4: the trim
/// ring is the single densest plane and empirically holds 45–55 % of the group
/// across resolutions, while a contamination band or a stray sub-cluster never
/// reaches this share. See DL-52.
const CIRCLE_INLIER_FRACTION: f64 = 0.4;

/// A robust circle fit must additionally see its inliers cover the full ring:
/// the largest angular gap between consecutive (by angle) inliers must be below
/// this many radians. A genuine closed trim circle has no large void; a partial
/// arc or a stray planar sub-cluster does. See DL-52.
const CIRCLE_MAX_ANGULAR_GAP: f64 = std::f64::consts::PI / 2.0;

/// A circle-fit group is accepted as a **single arc** when, after removing its
/// one open side (the largest angular gap — for a clipped seam this is the
/// portion outside the solid), every remaining inter-sample angular gap stays
/// within `ARC_GAP_FACTOR ×` the median gap. The spatial analogue of
/// [`CHAIN_GAP_FACTOR`]: a second large gap means two disjoint arcs share the
/// signature, which must chain per-arc instead. See DL-64.
const ARC_GAP_FACTOR: f64 = 6.0;

/// Fraction of a consolidated chain's samples that must lie on each branch's
/// analytic surface for the phantom-signature filter to keep it. A genuine
/// edge has ~all samples on both faces; a contributor-eps phantom pairing has
/// ~none. The slack below 1.0 absorbs the handful of extraction-noise outliers
/// near a curved seam's bounding triple-points, where the membership tolerance
/// (one post-dedup sample spacing) collapses below the local projection
/// accuracy. See DL-64.
const SURFACE_MEMBERSHIP_FRACTION: f64 = 0.85;

/// Fraction of a circle-fit group's inliers that must lie within `tol` of the
/// fitted radius for the fit to be accepted. A robust quantile in place of a
/// strict max residual: a densely-sampled curved seam collapses `tol` below the
/// tangent-plane extraction noise, so a handful of worst-case points must not
/// veto an otherwise clean circle, while a genuine non-circle still fails the
/// share. See DL-64.
const CIRCLE_RADIAL_FRACTION: f64 = 0.9;

// ─── small vector helpers (f64): shared, see brep::vec3 ─────────────────────
use super::edge_trace::{edge_tangent, project_to_edge, trace_edge, Field, TraceParams, TracedEdge};
use super::vec3::{
    add as add3, dist, dist2, dot, norm as norm3, normalize, scale as scale3, sub,
};

/// Centroid of a point set.
fn centroid(pts: &[[f64; 3]]) -> [f64; 3] {
    let n = pts.len() as f64;
    let mut c = [0.0; 3];
    for p in pts {
        c = add3(c, *p);
    }
    scale3(c, 1.0 / n)
}

/// The 3×3 covariance matrix of `pts` about `c` (unnormalised — scale is
/// irrelevant for the eigenvectors we want).
fn covariance(pts: &[[f64; 3]], c: [f64; 3]) -> Matrix3<f64> {
    let mut m = Matrix3::zeros();
    for p in pts {
        let d = sub(*p, c);
        for i in 0..3 {
            for j in 0..3 {
                m[(i, j)] += d[i] * d[j];
            }
        }
    }
    m
}

// ─── global per-group primitive fits (DL-51) ─────────────────────────────────

/// Eigenvectors of a covariance matrix, sorted by **descending** eigenvalue.
/// Returns `(λ, v)` pairs.
fn sorted_eigen(cov: &Matrix3<f64>) -> [(f64, [f64; 3]); 3] {
    let eig = SymmetricEigen::new(*cov);
    let mut pairs: Vec<(f64, [f64; 3])> = (0..3)
        .map(|i| {
            let v = eig.eigenvectors.column(i);
            (eig.eigenvalues[i], [v[0], v[1], v[2]])
        })
        .collect();
    pairs.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
    [pairs[0], pairs[1], pairs[2]]
}

/// Try to fit a single straight line (total-least-squares: centroid + dominant
/// eigenvector) to all of a group's deduped points. On success returns the
/// points sorted by their projection onto the line direction and oriented
/// Hermite samples for one edge; on failure (max perpendicular residual exceeds
/// `tol`) returns `None`. See DL-51.
fn fit_line(points: &[EdgePoint], tol: f64) -> Option<Vec<HermiteSample>> {
    if points.len() < 2 {
        return None;
    }
    let pos: Vec<[f64; 3]> = points.iter().map(|p| p.position).collect();
    let c = centroid(&pos);
    let cov = covariance(&pos, c);
    let eig = sorted_eigen(&cov);
    let dir = eig[0].1;
    let dlen = norm3(dir);
    if dlen < 1e-12 {
        return None;
    }
    let dir = scale3(dir, 1.0 / dlen);

    // Max perpendicular residual: distance of each point from the line.
    let mut max_res = 0.0f64;
    for p in &pos {
        let v = sub(*p, c);
        let along = dot(v, dir);
        let perp = sub(v, scale3(dir, along));
        max_res = max_res.max(norm3(perp));
    }
    if max_res > tol {
        return None;
    }

    // Sort the *indices* by projection onto the line direction.
    let mut order: Vec<usize> = (0..points.len()).collect();
    order.sort_by(|&i, &j| {
        let ti = dot(sub(pos[i], c), dir);
        let tj = dot(sub(pos[j], c), dir);
        ti.partial_cmp(&tj).unwrap()
    });
    Some(orient_chain(points, &order))
}

/// Try to fit a single circle to a group's deduped points, **robust to an
/// axial contamination tail** (DL-52).
///
/// First a best-fit plane (PCA — the two dominant eigenvectors span it, the
/// smallest is the normal). The plane *offset* is taken at the **densest**
/// signed distance along the normal, not the centroid: a drilled-cylinder trim
/// group carries extra cylinder-wall samples one or two cells deep along the
/// bore axis (same signature, different axial offset), and the centroid is
/// dragged off the true ring by that tail — the dominant offset is not. Points
/// within `tol` of that plane are the **inliers**; we run the algebraic (Kåsa)
/// circle fit on the inliers alone and accept when they (a) are a clear majority
/// ([`CIRCLE_INLIER_FRACTION`]), (b) lie within `tol` of the fitted radius for a
/// strong majority ([`CIRCLE_RADIAL_FRACTION`]), and (c) form **one contiguous
/// angular run** — either a full ring (largest gap below
/// [`CIRCLE_MAX_ANGULAR_GAP`]) or a single arc clipped out of a circle, with no
/// *second* large gap ([`ARC_GAP_FACTOR`]). A full ring returns sorted as a
/// closed loop; a clipped arc returns as an OPEN chain (DL-64) whose ends snap
/// to the bounding vertices. A straight edge, a two-arc signature, or a stray
/// sub-cluster fails a gate and falls back to gap-chaining. See DL-51, DL-52,
/// DL-64.
fn fit_circle(points: &[EdgePoint], tol: f64) -> Option<Vec<HermiteSample>> {
    // A circle needs enough distinct points to be well-posed and to read as a
    // loop downstream.
    if points.len() < 8 {
        return None;
    }
    let pos: Vec<[f64; 3]> = points.iter().map(|p| p.position).collect();
    let c = centroid(&pos);
    let cov = covariance(&pos, c);
    let eig = sorted_eigen(&cov);
    let u = eig[0].1; // in-plane axis 1
    let v = eig[1].1; // in-plane axis 2
    let normal = eig[2].1; // plane normal (smallest spread)

    let nlen = norm3(normal);
    if nlen < 1e-12 {
        return None;
    }
    let normal = scale3(normal, 1.0 / nlen);

    // Robust plane offset: the **densest** signed distance along the normal,
    // not the centroid/median. A drilled-cylinder trim group is the true ring
    // (one dense plane) plus a thinner axial contamination band at other
    // offsets; the centroid and even the median can land between the two, so we
    // mode-seek instead — for each point's offset, count how many offsets fall
    // within `tol`, and keep the offset (and its inliers) with the largest
    // count. The true ring, being the densest single plane, wins.
    let along: Vec<f64> = pos.iter().map(|p| dot(sub(*p, c), normal)).collect();
    let mut best_off = along[0];
    let mut best_count = 0usize;
    for &o in &along {
        let count = along.iter().filter(|&&q| (q - o).abs() <= tol).count();
        if count > best_count {
            best_count = count;
            best_off = o;
        }
    }
    let plane_off = best_off;

    // Inliers: points within `tol` of the dominant plane.
    let inlier_idx: Vec<usize> = (0..points.len())
        .filter(|&i| (along[i] - plane_off).abs() <= tol)
        .collect();
    if (inlier_idx.len() as f64) < CIRCLE_INLIER_FRACTION * points.len() as f64 {
        return None;
    }
    if inlier_idx.len() < 8 {
        return None;
    }

    // Project inliers into the (u, v) plane coordinates and run a Kåsa fit:
    // minimise Σ (x²+y² − (a·x + b·y + d))², solving the 3×3 normal equations.
    let uv: Vec<(f64, f64)> = inlier_idx
        .iter()
        .map(|&i| {
            let d = sub(pos[i], c);
            (dot(d, u), dot(d, v))
        })
        .collect();
    let mut m = Matrix3::zeros();
    let mut rhs = Vector3::zeros();
    for &(x, y) in &uv {
        let z = x * x + y * y;
        let row = [x, y, 1.0];
        for i in 0..3 {
            for j in 0..3 {
                m[(i, j)] += row[i] * row[j];
            }
            rhs[i] += row[i] * z;
        }
    }
    let sol: Vector3<f64> = m.lu().solve(&rhs)?;
    let (a, b, d) = (sol[0], sol[1], sol[2]);
    // x² + y² = a·x + b·y + d ⇒ centre (a/2, b/2), r² = d + (a²+b²)/4.
    let cx = a / 2.0;
    let cy = b / 2.0;
    let r2 = d + cx * cx + cy * cy;
    if r2 <= 0.0 {
        return None;
    }
    let r = r2.sqrt();

    // Radial fit quality: a *strong majority* of inliers must lie within `tol`
    // of the fitted radius. A strict max-residual test collapses for a densely
    // sampled curved seam — there `tol` (= one post-dedup sample spacing × 1.5)
    // falls below the first-order tangent-plane extraction noise, so a few
    // worst-case points near the seam would reject an otherwise perfect circle
    // (DL-64). A genuine non-circle (an L of two segments, a stray cluster) has
    // *many* points far from any single radius and still fails this share.
    let within = uv
        .iter()
        .filter(|&&(x, y)| (((x - cx).powi(2) + (y - cy).powi(2)).sqrt() - r).abs() <= tol)
        .count();
    if (within as f64) < CIRCLE_RADIAL_FRACTION * uv.len() as f64 {
        return None;
    }

    // Order inliers by angle about the fitted centre and inspect the angular
    // gaps. A genuine circular edge is one contiguous angular run: a **full
    // ring** (every gap small) or a **single arc** clipped out of a circle —
    // e.g. a sphere ∩ box-face seam, whose missing portion lies outside the
    // solid (DL-64). Both are one edge; only a signature carrying *two or more*
    // disjoint arcs (two large gaps) must fall back to per-arc gap-chaining.
    let mut order: Vec<usize> = (0..inlier_idx.len()).collect();
    let angle = |k: usize| (uv[k].1 - cy).atan2(uv[k].0 - cx);
    order.sort_by(|&i, &j| angle(i).partial_cmp(&angle(j)).unwrap());

    // Consecutive angular gaps along the sorted order, plus the wrap gap. Track
    // the two largest and where the single largest sits (it is the arc's open
    // side; for a full ring it is merely the largest of many tiny gaps).
    let n = order.len();
    let gap_at = |i: usize| -> f64 {
        if i + 1 < n {
            angle(order[i + 1]) - angle(order[i])
        } else {
            // wrap from last back to first (+2π)
            angle(order[0]) + std::f64::consts::TAU - angle(order[n - 1])
        }
    };
    let (mut max_gap, mut max_idx, mut second_gap) = (0.0f64, n - 1, 0.0f64);
    for i in 0..n {
        let g = gap_at(i);
        if g > max_gap {
            second_gap = max_gap;
            max_gap = g;
            max_idx = i;
        } else if g > second_gap {
            second_gap = g;
        }
    }

    // A median-relative continuity bound: after removing the single open side,
    // every remaining gap must stay within `ARC_GAP_FACTOR ×` the median gap,
    // mirroring the spatial gap-chaining rule. Two disjoint arcs leave a second
    // large gap and are rejected here so each arc chains on its own.
    let mut gaps: Vec<f64> = (0..n).map(gap_at).collect();
    gaps.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median_gap = gaps[n / 2];
    if second_gap > (median_gap * ARC_GAP_FACTOR).max(CIRCLE_MAX_ANGULAR_GAP) {
        return None;
    }

    let inlier_points: Vec<EdgePoint> = inlier_idx.iter().map(|&i| points[i]).collect();
    if max_gap <= CIRCLE_MAX_ANGULAR_GAP {
        // Full ring → one closed loop. orient_chain references k+1 for the
        // forward direction; the last sample should look back to the first, so
        // we append the first index to wrap, then drop the duplicate sample.
        let mut closed: Vec<usize> = order.clone();
        closed.push(order[0]);
        let mut samples = orient_chain(&inlier_points, &closed);
        samples.pop();
        Some(samples)
    } else {
        // Single clipped arc → one OPEN chain. Rotate the angular order so it
        // starts just after the open side (`max_idx` → `max_idx + 1`); the
        // chain then runs end-to-end along the arc and its two endpoints snap
        // to the bounding vertices downstream.
        let rotated: Vec<usize> = (0..n).map(|k| order[(max_idx + 1 + k) % n]).collect();
        Some(orient_chain(&inlier_points, &rotated))
    }
}

// ─── general SDF edge tracer wiring (DL-68) ──────────────────────────────────

/// Largest multiple of the local sample spacing a seed may drift when projected
/// onto the joint zero-set before we reject it as *not on this edge*. A genuine
/// seam sample sits within roughly one octree cell of the true edge; a phantom
/// signature's sample (e.g. a cylinder-wall point tagged with a non-touching box
/// face) projects far away or not at all, and is dropped here rather than seeding
/// a spurious component. See DL-68, and the membership backstop
/// [`chain_on_both_surfaces`] (DL-52).
const SEAM_SEED_DRIFT_FACTOR: f64 = 4.0;

/// A traced component consumes every remaining sample within this multiple of
/// spacing of its polyline; the rest seed the next disjoint component (one
/// signature can carry several, e.g. the two circles of a sphere ∩ cylinder).
const SEAM_NEAR_FACTOR: f64 = 1.5;

/// Safety cap on disjoint components traced from one signature group — far above
/// any real fan-out, a backstop against a non-terminating seed loop.
const SEAM_MAX_COMPONENTS: usize = 16;

/// A traced component's arc length may not exceed this multiple of the group's
/// spatial extent (bounding-box diagonal). A circle's circumference is π× its
/// diameter, a line ≤ its span, so 4× clears any real single seam while a
/// runaway on an unbounded edge stalls quickly into the sample-fit fallback.
const SEAM_MAX_ARC_FACTOR: f64 = 4.0;

/// The analytic scalar [`Field`] backing a branch, or `None` for a branch with
/// no analytic surface (a curved sketch-sweep wall). Every primitive face —
/// box plane, sphere, (infinite) cylinder, cone, torus, and the analytic
/// extrude/revolve walls — appears in [`branch_halfspace_map`], so the tracer
/// covers the whole analytic surface zoo and their cross-pairs; the leftover
/// curved sketch walls fall through to the sampled fits.
fn branch_field(
    halfspaces: &HashMap<u32, (fielddraft_protocol::HalfSpace, fielddraft_protocol::Transform)>,
    branch: u32,
) -> Option<Field<'static>> {
    let (hs, xf) = halfspaces.get(&branch)?;
    Some(Field::Analytic {
        hs: hs.clone(),
        xf: *xf,
    })
}

/// Is `q` within `tol` of any vertex of the polyline?
fn near_polyline(q: [f64; 3], poly: &[[f64; 3]], tol: f64) -> bool {
    let t2 = tol * tol;
    poly.iter().any(|&p| dist2(p, q) <= t2)
}

/// Does a candidate vertex genuinely bound a seam — i.e. is it a *real* junction
/// where **all** its branch surfaces actually pass through, not a coarse-octree
/// contributor-bleed artifact?
///
/// At coarse resolution a smooth seam (e.g. a sphere cap circle on a box face)
/// can spuriously activate a *non-touching* third face inside an octree cell, so
/// `dedup_vertices` mints a phantom triple vertex sitting *on the seam* but whose
/// third surface is a whole feature away. Feeding it to the tracer would chop the
/// smooth seam into arcs at that fake corner. A real corner satisfies every
/// branch's analytic surface to within a fraction of the local spacing (it was
/// refined to the common zero by [`refine_triple_vertex`]); a phantom one misses
/// its bleed branch by `O(feature size)`. A branch with no analytic surface (a
/// tessellated sweep wall) cannot be disproven, so it is kept conservatively.
fn vertex_bounds_seam(
    halfspaces: &HashMap<u32, (fielddraft_protocol::HalfSpace, fielddraft_protocol::Transform)>,
    branches: &[BranchId],
    pos: [f64; 3],
    spacing: f64,
) -> bool {
    let tol = (0.5 * spacing).max(2.0 * VERTEX_REFINE_TOL as f64);
    let p = [pos[0] as f32, pos[1] as f32, pos[2] as f32];
    branches.iter().all(|b| match halfspaces.get(&b.as_u32()) {
        None => true, // non-analytic branch: cannot judge → keep
        Some((hs, xf)) => {
            (crate::csg::sdf_halfspace(hs, crate::world_to_local(xf, p)).abs() as f64) <= tol
        }
    })
}

/// Convert a [`TracedEdge`] into oriented Hermite samples: each tangent is the
/// exact edge tangent `∇f_a × ∇f_b` at that point (falling back to the chord
/// only at a degeneracy), flipped to run forward along the polyline.
fn traced_to_hermite(fa: &Field, fb: &Field, traced: &TracedEdge) -> Vec<HermiteSample> {
    let pts = &traced.points;
    let n = pts.len();
    let mut out = Vec::with_capacity(n);
    for k in 0..n {
        let p = pts[k];
        // Forward chord direction at this sample, for orientation.
        let fwd = if k + 1 < n {
            sub(pts[k + 1], p)
        } else if traced.closed {
            sub(pts[0], p)
        } else if k > 0 {
            sub(p, pts[k - 1])
        } else {
            [0.0; 3]
        };
        let mut t = edge_tangent(fa, fb, p).unwrap_or_else(|| normalize(fwd));
        if dot(t, fwd) < 0.0 {
            t = scale3(t, -1.0);
        }
        out.push(HermiteSample { point: p, tangent: t });
    }
    out
}

/// Reconstruct one signature group's seam(s) with the general SDF edge tracer
/// (DL-68), replacing DL-67's per-pair closed-form circle.
///
/// For each disjoint component, a central sample seeds [`trace_edge`]: project
/// it exactly onto `{f_a = 0 ∧ f_b = 0}`, then march the tangent in both
/// directions, closing a loop or snapping each end to a bounding triple vertex.
/// The component's covered samples are removed and any leftover cluster seeds the
/// next component. Resolution-independent and surface-exact for the whole
/// analytic surface zoo; returns `None` (→ the sampled fits) when no seed sits on
/// a well-conditioned edge — a phantom signature, or a pair that does not meet.
fn trace_seam_chains(
    fa: &Field,
    fb: &Field,
    pts: &[[f64; 3]],
    bounding: &[[f64; 3]],
    spacing: f64,
) -> Option<Vec<Chain>> {
    if pts.len() < 3 {
        return None;
    }
    let mut params = TraceParams::from_step(spacing);
    // Arc budget: a real seam component cannot be longer than a small multiple
    // of the group's spatial extent. Beyond that the march is on an unbounded
    // edge (a stalled, non-clean end) and we hand the whole group to the proven
    // sample fits rather than emit a runaway chain.
    params.max_arc = SEAM_MAX_ARC_FACTOR * group_span(pts);
    let drift2 = (SEAM_SEED_DRIFT_FACTOR * spacing).powi(2);
    let near_tol = SEAM_NEAR_FACTOR * spacing;
    let mut remaining: Vec<[f64; 3]> = pts.to_vec();
    let mut chains: Vec<Chain> = Vec::new();

    for _ in 0..SEAM_MAX_COMPONENTS {
        if remaining.is_empty() {
            break;
        }
        // Seed from the sample nearest the centroid of what is left — the most
        // interior point, least likely to be a noisy fragment end.
        let c = centroid(&remaining);
        let si = (0..remaining.len())
            .min_by(|&i, &j| {
                dist2(remaining[i], c)
                    .partial_cmp(&dist2(remaining[j], c))
                    .unwrap()
            })
            .unwrap();
        let seed = remaining[si];

        // Drop a seed that is not actually on this edge (phantom / non-meeting
        // pair): its projection must stay within a few cells. Always removes the
        // seed, so the loop makes progress.
        let on_edge = project_to_edge(fa, fb, seed).is_some_and(|proj| dist2(seed, proj) <= drift2);
        if !on_edge {
            remaining.swap_remove(si);
            continue;
        }
        let Some(traced) = trace_edge(fa, fb, seed, bounding, &params) else {
            remaining.swap_remove(si);
            continue;
        };

        // The tracer must settle the topology *cleanly*: a closed ring, or an
        // open arc that reached a real bounding vertex at BOTH ends. A stalled
        // end (ran out of arc budget, projection diverged, tangent collapsed)
        // means this pair is not cleanly traceable here — abandon the tracer for
        // the whole group and fall back to the sample fits, which are robust to
        // exactly these ill-conditioned signatures.
        let clean = traced.closed || (traced.ends.0.is_some() && traced.ends.1.is_some());
        if !clean {
            return None;
        }

        // Consume the seed and every sample belonging to this component; the
        // rest seed the next disjoint component. Membership is by **projection**,
        // not raw proximity: an octree sample can sit up to a cell off the true
        // seam, far more than the dense polyline's point spacing, yet it still
        // *projects onto* this curve. Testing the projection (which lands exactly
        // on the traced component, or elsewhere for a genuinely disjoint one)
        // removes the off-curve stragglers that would otherwise re-seed a
        // duplicate of the same loop. Removing the seed by value guarantees
        // progress even if its own projection somehow misses the polyline.
        remaining.retain(|&q| {
            if q == seed {
                return false;
            }
            match project_to_edge(fa, fb, q) {
                Some(pq) => !near_polyline(pq, &traced.points, near_tol),
                None => true,
            }
        });

        let samples = traced_to_hermite(fa, fb, &traced);
        if samples.len() >= 2 {
            // The tracer already settled the topology: a closed ring needs no
            // bounding vertices, an open arc ends exactly on its two; carry that
            // so the emitter skips the sample-fit snap/loop heuristics.
            chains.push(Chain {
                samples,
                closed: Some(traced.closed),
            });
        }
    }

    (!chains.is_empty()).then_some(chains)
}

// ─── vertex dedup ───────────────────────────────────────────────────────────

/// A deduplicated vertex: the canonical branch-set plus the centroid of all
/// vertex-cell positions that shared it.
struct DedupVertex {
    branches: Vec<BranchId>,
    position: [f64; 3],
}

/// Deduplicate the Phase-3 vertices by branch-set, averaging positions. Many
/// `Vertex` cells around one corner emit the same branch-triple; we merge them
/// into one vertex at the centroid (DL-13).
fn dedup_vertices(raw: &[Vertex]) -> Vec<DedupVertex> {
    // Keyed by the branch-set's raw u32 payloads (already sorted by
    // `Vertex::new`), preserving insertion order for determinism.
    let mut order: Vec<Vec<u32>> = Vec::new();
    let mut acc: HashMap<Vec<u32>, ([f64; 3], usize, Vec<BranchId>)> = HashMap::new();
    for v in raw {
        let key: Vec<u32> = v.branches.iter().map(|b| b.as_u32()).collect();
        let entry = acc.entry(key.clone()).or_insert_with(|| {
            order.push(key.clone());
            ([0.0; 3], 0, v.branches.clone())
        });
        entry.0[0] += v.position[0];
        entry.0[1] += v.position[1];
        entry.0[2] += v.position[2];
        entry.1 += 1;
    }
    order
        .into_iter()
        .map(|key| {
            let (sum, n, branches) = &acc[&key];
            let inv = 1.0 / *n as f64;
            DedupVertex {
                branches: branches.clone(),
                position: [sum[0] * inv, sum[1] * inv, sum[2] * inv],
            }
        })
        .collect()
}

/// Maximum Gauss-Newton iterations when re-solving an analytic triple vertex.
const VERTEX_REFINE_MAX_ITER: usize = 24;
/// Convergence threshold on the largest per-surface signed-distance residual
/// for accepting a refined analytic triple vertex (world units, matching the
/// extractor's `ACCEPT_EPS` spirit). A near-tangential corner whose surfaces do
/// not truly meet leaves the residual above this and is rejected (keep centroid).
const VERTEX_REFINE_TOL: f32 = 1e-3;

/// Re-solve a multi-branch vertex as the exact common zero of its branches'
/// analytic surfaces, by Gauss-Newton from the centroid `seed` (DL-66).
///
/// At a *near-tangential* triple junction — e.g. a sphere grazing a box edge —
/// the octree tags a fat band of `Vertex` cells (three branches active across an
/// extended sliver) and the per-cell tangent-plane LSQ in [`extract_vertex`]
/// scatters wildly, including gross outliers where one branch's seed landed on
/// the wrong side. [`dedup_vertices`] averages those, so the corner drifts off
/// the true intersection; the converging edge arcs then no longer snap to it and
/// every touching face falls back to an unordered loop.
///
/// The three surfaces themselves meet cleanly there — two box planes + a sphere
/// are well-conditioned at the corner; the tangency is between the *seam* and
/// the box edge, not between the surface normals — so we solve the corner
/// directly: minimise `Σ sdf_i(p)²` over the analytic branch surfaces by
/// Gauss-Newton. Resolution-independent and outlier-proof: the scattered samples
/// only seed it. Returns `None` (keep the centroid) when any branch has no
/// analytic surface (a tessellated sweep / domain-warp face), the normals are
/// rank-deficient (no isolated corner), or the solve does not converge.
fn refine_triple_vertex(
    halfspaces: &HashMap<u32, (fielddraft_protocol::HalfSpace, fielddraft_protocol::Transform)>,
    branches: &[BranchId],
    seed: [f64; 3],
) -> Option<[f64; 3]> {
    // Every branch must carry an analytic surface, else we cannot solve.
    let surfs: Vec<&(fielddraft_protocol::HalfSpace, fielddraft_protocol::Transform)> =
        branches.iter().map(|b| halfspaces.get(&b.as_u32())).collect::<Option<_>>()?;
    if surfs.len() < 3 {
        return None;
    }

    // Signed distance of world point `p` to one branch surface.
    let field = |hs, xf, p: [f32; 3]| crate::csg::sdf_halfspace(hs, crate::world_to_local(xf, p));
    let residual = |p: [f32; 3]| -> f32 {
        surfs.iter().map(|(hs, xf)| field(hs, xf, p).abs()).fold(0.0, f32::max)
    };

    let mut p = [seed[0] as f32, seed[1] as f32, seed[2] as f32];
    let h = 1e-3f32; // central-difference step (planes are exact; only the
    // curved surfaces incur a step error, which Gauss-Newton absorbs).

    for _ in 0..VERTEX_REFINE_MAX_ITER {
        if residual(p) < VERTEX_REFINE_TOL {
            return Some([p[0] as f64, p[1] as f64, p[2] as f64]);
        }
        let mut jtj = Matrix3::<f32>::zeros();
        let mut jtf = Vector3::<f32>::zeros();
        for (hs, xf) in &surfs {
            let f = field(hs, xf, p);
            let mut g = [0.0f32; 3];
            for (a, gi) in g.iter_mut().enumerate() {
                let (mut pp, mut pm) = (p, p);
                pp[a] += h;
                pm[a] -= h;
                *gi = (field(hs, xf, pp) - field(hs, xf, pm)) / (2.0 * h);
            }
            let gv = Vector3::new(g[0], g[1], g[2]);
            jtj += gv * gv.transpose();
            jtf += gv * f;
        }
        // Reject a rank-deficient normal matrix (parallel normals → no isolated
        // corner): the smallest eigenvalue of `JᵀJ` collapses there.
        let min_ev = SymmetricEigen::new(jtj)
            .eigenvalues
            .iter()
            .copied()
            .fold(f32::INFINITY, f32::min);
        if min_ev < 1e-6 {
            return None;
        }
        let delta = jtj.try_inverse()? * jtf;
        p = [p[0] - delta[0], p[1] - delta[1], p[2] - delta[2]];
    }
    (residual(p) < VERTEX_REFINE_TOL).then_some([p[0] as f64, p[1] as f64, p[2] as f64])
}

// ─── chaining ─────────────────────────────────────────────────────────────

/// One ordered run of edge points (a contiguous chain along one edge).
struct Chain {
    samples: Vec<HermiteSample>,
    /// Topology decided by the DL-68 tracer: `Some(true)` = closed ring (no
    /// bounding vertices), `Some(false)` = open arc ending exactly on its two
    /// bounding vertices. `None` for a sample-fit / gap-chained run, whose
    /// endpoints go through the legacy snap + loop-detection heuristics.
    closed: Option<bool>,
}

/// Merge coincident edge points (DL-17). Phase 3 emits one edge point per
/// `Edge` cell, and several neighbouring cells project to the *same* analytic
/// point on a straight edge — so the raw group is riddled with exact (or
/// near-exact) duplicates. We collapse points closer than `eps` into one
/// representative (averaged position + tangent). Returns the deduped points and
/// the characteristic spacing of the *distinct* survivors. `eps` is chosen
/// from the smallest non-zero pairwise distance so it scales with the octree
/// resolution without any external parameter.
fn dedup_points(points: &[EdgePoint]) -> (Vec<EdgePoint>, f64) {
    // The dedup `eps` is seeded from the group's spatial **span** (its bounding
    // diagonal), not from a nearest-neighbour statistic. A straight extrude
    // wall stacks many octree cells along the sweep axis; some project to the
    // *exact* same analytic point and others to two near-coincident points
    // separated only by float noise (~1e-8). A median NN distance then
    // collapses to 0 and a min NN distance collapses to the noise floor — both
    // useless as an eps. A small fraction of the span is robust to either: it
    // sits far below any real inter-sample step (span / cells_per_edge) yet far
    // above float noise, so duplicates and noise pairs merge while genuine
    // samples survive (DL-51, fixing the previous NN-seed dropout that silently
    // discarded such edges).
    let positions: Vec<[f64; 3]> = points.iter().map(|p| p.position).collect();
    let span = group_span(&positions);
    if span <= 0.0 {
        // All points coincide (or a single point) → one representative.
        return (points.first().copied().into_iter().collect(), 0.0);
    }
    let eps = span * 1e-3;
    let eps2 = eps * eps;

    let mut reps: Vec<([f64; 3], [f64; 3], usize)> = Vec::new(); // (Σpos, Σtan, n)
    'pt: for ep in points {
        for rep in reps.iter_mut() {
            let centroid = [
                rep.0[0] / rep.2 as f64,
                rep.0[1] / rep.2 as f64,
                rep.0[2] / rep.2 as f64,
            ];
            if dist2(ep.position, centroid) <= eps2 {
                rep.0 = [
                    rep.0[0] + ep.position[0],
                    rep.0[1] + ep.position[1],
                    rep.0[2] + ep.position[2],
                ];
                // Orient the tangent to agree before averaging (tangents are
                // sign-ambiguous; n_a × n_b may flip cell to cell).
                let t = if dot(ep.tangent, rep.1) < 0.0 {
                    [-ep.tangent[0], -ep.tangent[1], -ep.tangent[2]]
                } else {
                    ep.tangent
                };
                rep.1 = [rep.1[0] + t[0], rep.1[1] + t[1], rep.1[2] + t[2]];
                rep.2 += 1;
                continue 'pt;
            }
        }
        reps.push((ep.position, ep.tangent, 1));
    }

    let sig = points[0].signature;
    let merged: Vec<EdgePoint> = reps
        .iter()
        .map(|(sp, st, n)| {
            let inv = 1.0 / *n as f64;
            let mut t = [st[0] * inv, st[1] * inv, st[2] * inv];
            let tn = dot(t, t).sqrt();
            if tn > 0.0 {
                t = [t[0] / tn, t[1] / tn, t[2] / tn];
            }
            EdgePoint {
                position: [sp[0] * inv, sp[1] * inv, sp[2] * inv],
                tangent: t,
                signature: sig,
            }
        })
        .collect();

    // Recompute spacing on the deduped survivors (now free of duplicate /
    // noise pairs after the span-relative merge). The median positive NN
    // distance is the characteristic inter-sample step used to scale the fit /
    // gap / snap thresholds.
    let merged_pos: Vec<[f64; 3]> = merged.iter().map(|p| p.position).collect();
    let spacing = median_positive_spacing(&merged_pos);
    (merged, spacing)
}

/// The spatial **span** (bounding-box diagonal) of a point set — used to seed a
/// duplicate-count-robust dedup `eps` (a small fraction of the span). Returns 0
/// for an empty set or a single coincident cluster.
fn group_span(points: &[[f64; 3]]) -> f64 {
    if points.len() < 2 {
        return 0.0;
    }
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for p in points {
        for a in 0..3 {
            lo[a] = lo[a].min(p[a]);
            hi[a] = hi[a].max(p[a]);
        }
    }
    norm3(sub(hi, lo))
}

/// Median of the **strictly positive** nearest-neighbour distances across a
/// point set — the characteristic local sample spacing used to scale the
/// gap/snap/loop thresholds. Zero distances (coincident duplicates) are
/// excluded so the spacing reflects the real inter-sample step, and the median
/// (not the mean) is robust to a few large gaps.
fn median_positive_spacing(points: &[[f64; 3]]) -> f64 {
    if points.len() < 2 {
        return 0.0;
    }
    let mut nn: Vec<f64> = Vec::with_capacity(points.len());
    for (i, &p) in points.iter().enumerate() {
        let mut best = f64::INFINITY;
        for (j, &q) in points.iter().enumerate() {
            if i != j {
                let d2 = dist2(p, q);
                if d2 > 0.0 {
                    best = best.min(d2);
                }
            }
        }
        if best.is_finite() {
            nn.push(best.sqrt());
        }
    }
    if nn.is_empty() {
        return 0.0;
    }
    nn.sort_by(|a, b| a.partial_cmp(b).unwrap());
    nn[nn.len() / 2]
}

/// Greedy nearest-neighbour walk over the edge points of one signature group,
/// split into chains where the NN gap exceeds `gap_threshold`. Tangents are
/// oriented to point forward along the walk (DL-11, DL-16).
fn chain_group(points: &[EdgePoint], gap_threshold: f64) -> Vec<Chain> {
    let n = points.len();
    let mut used = vec![false; n];
    let mut chains = Vec::new();

    // Find an unused spatial extreme (smallest x, then y, then z) as a start
    // seed so a single edge yields a single chain from one true end.
    let pick_seed = |used: &[bool]| -> Option<usize> {
        let mut best: Option<usize> = None;
        for (i, &u) in used.iter().enumerate() {
            if u {
                continue;
            }
            let p = points[i].position;
            best = match best {
                None => Some(i),
                Some(b) => {
                    let q = points[b].position;
                    if p < q {
                        Some(i)
                    } else {
                        Some(b)
                    }
                }
            };
        }
        best
    };

    while let Some(seed) = pick_seed(&used) {
        used[seed] = true;
        let mut idx_chain = vec![seed];
        // Grow forward by repeated nearest-neighbour, breaking on a big gap.
        loop {
            let cur = *idx_chain.last().unwrap();
            let cur_pos = points[cur].position;
            let mut nearest: Option<(usize, f64)> = None;
            for (i, &u) in used.iter().enumerate() {
                if u {
                    continue;
                }
                let d2 = dist2(cur_pos, points[i].position);
                nearest = match nearest {
                    None => Some((i, d2)),
                    Some((_, bd)) if d2 < bd => Some((i, d2)),
                    other => other,
                };
            }
            match nearest {
                Some((i, d2)) if d2.sqrt() <= gap_threshold => {
                    used[i] = true;
                    idx_chain.push(i);
                }
                _ => break,
            }
        }

        // Build oriented Hermite samples: flip each tangent to agree with the
        // local forward direction (previous→next), so the chain is monotone.
        let samples = orient_chain(points, &idx_chain);
        chains.push(Chain {
            samples,
            closed: None,
        });
    }

    chains
}

/// Turn an ordered index list into oriented Hermite samples. Each tangent is
/// flipped to point along the walk (using the segment to the next point, or
/// the previous segment for the last sample). See DL-16.
fn orient_chain(points: &[EdgePoint], idx_chain: &[usize]) -> Vec<HermiteSample> {
    let m = idx_chain.len();
    let mut out = Vec::with_capacity(m);
    for k in 0..m {
        let p = points[idx_chain[k]].position;
        let mut t = points[idx_chain[k]].tangent;
        // Reference forward direction along the chain at this sample.
        let fwd = if k + 1 < m {
            sub(points[idx_chain[k + 1]].position, p)
        } else if k > 0 {
            sub(p, points[idx_chain[k - 1]].position)
        } else {
            // Single-point chain: nothing to orient against, keep as-is.
            [0.0; 3]
        };
        if dot(t, fwd) < 0.0 {
            t = [-t[0], -t[1], -t[2]];
        }
        out.push(HermiteSample {
            point: p,
            tangent: t,
        });
    }
    out
}

// ─── phantom-signature filter (DL-52) ───────────────────────────────────────

/// Does the body contain a domain warp (Twist / Bend / Displace)? Under a warp
/// the analytic branch→half-space map describes the *un-warped* primitive, so
/// it does not match the real (warped) surface and must NOT be used to validate
/// edges — the phantom filter is disabled for such bodies. See DL-52.
fn body_has_warp(node: &BoundedCsg) -> bool {
    match node {
        BoundedCsg::Twist { .. } | BoundedCsg::Bend { .. } | BoundedCsg::Displace { .. } => true,
        BoundedCsg::Union { left, right, .. }
        | BoundedCsg::Difference { left, right, .. }
        | BoundedCsg::Intersection { left, right, .. }
        | BoundedCsg::SmoothUnion { left, right, .. }
        | BoundedCsg::SmoothDifference { left, right, .. }
        | BoundedCsg::MaskedBlend { left, right, .. }
        | BoundedCsg::Morph { left, right, .. } => body_has_warp(left) || body_has_warp(right),
        BoundedCsg::Shell { inner, .. } => body_has_warp(inner),
        BoundedCsg::Leaf { .. }
        | BoundedCsg::Extrude { .. }
        | BoundedCsg::Revolve { .. }
        | BoundedCsg::Empty => false,
    }
}

/// A real B-Rep edge between two analytic faces lies on **both** faces. A
/// subtracted curved primitive (a drilled cylinder), however, makes the
/// extractor mint *phantom* signature groups: octree cells on the cylinder wall
/// that pass within the contributor `eps` of a box face inherit that face's
/// branch, so a `(box_face, cyl_wall)` group appears whose points sit on the
/// cylinder wall but **nowhere near** the box face (e.g. a wall ring at radius
/// 0.5 paired with the `x = 1` box plane). Such a pair is not a real edge.
///
/// `surface_off_factor`-scaled membership test: a group survives only if a
/// strong majority of its points lie on *both* branches' analytic surfaces
/// (`|sdf_halfspace| ≤ tol`). When either branch has **no** analytic entry
/// (a tessellated sketch-sweep wall, DL-25/DL-50), the test is skipped for that
/// branch — we cannot validate it and must not drop legitimate edges. See
/// DL-52.
fn chain_on_both_surfaces(
    halfspaces: &HashMap<u32, (fielddraft_protocol::HalfSpace, fielddraft_protocol::Transform)>,
    sa: u32,
    sb: u32,
    samples: &[HermiteSample],
    tol: f64,
) -> bool {
    if samples.is_empty() {
        return true;
    }
    let check_branch = |branch: u32| -> bool {
        let Some((hs, xf)) = halfspaces.get(&branch) else {
            // No analytic surface to validate against → cannot judge, keep.
            return true;
        };
        // A *strong majority* of a consolidated chain's samples must lie on this
        // branch's analytic surface. A phantom pairing — whose run sits entirely
        // off one face by a cell-scale offset (e.g. a cylinder-wall ring at the
        // bore radius) — has essentially *no* on-surface samples and is rejected
        // wholesale, while a real edge has all but a few. The few exceptions are
        // extraction-noise outliers near the bounding triple-points, where a
        // densely-sampled curved seam collapses `tol` (= one post-dedup sample
        // spacing) below the local projection accuracy; demanding unanimity there
        // would discard the whole genuine seam over a handful of borderline end
        // samples (DL-64).
        let on = samples
            .iter()
            .filter(|s| {
                let pw = [s.point[0] as f32, s.point[1] as f32, s.point[2] as f32];
                let local = crate::world_to_local(xf, pw);
                (crate::csg::sdf_halfspace(hs, local).abs() as f64) <= tol
            })
            .count();
        on as f64 >= SURFACE_MEMBERSHIP_FRACTION * samples.len() as f64
    };
    check_branch(sa) && check_branch(sb)
}

// ─── public entry point ───────────────────────────────────────────────────

/// Build a **wireframe** [`BRep`] from `body`'s classified `octree`: ordered
/// Hermite edge chains bounded by deduplicated vertices.
///
/// Runs Phase 3 ([`extract_features`]) internally, deduplicates vertices by
/// branch-set, chains the per-signature edge points by greedy spatial
/// adjacency (splitting on large gaps), snaps chain ends to candidate
/// vertices, and emits one [`EdgeGeometry::Hermite`] [`Edge`] per chain.
/// Faces / shells / solids are left empty (C.6).
pub fn build_wireframe(body: &BoundedCsg, octree: &Octree) -> BRep {
    let (edge_points, raw_vertices) = extract_features(body, octree);
    let mut dedup = dedup_vertices(&raw_vertices);

    // Re-solve each multi-branch vertex as the exact analytic surface
    // intersection, seeded from its centroid (DL-66). This snaps near-tangential
    // triple junctions (a sphere grazing a box edge) back onto the true corner,
    // where the scattered per-cell LSQ otherwise drifts the centroid off it.
    let vertex_halfspaces = crate::brep::weaving::branch_halfspace_map(body);
    for dv in &mut dedup {
        if let Some(p) = refine_triple_vertex(&vertex_halfspaces, &dv.branches, dv.position) {
            dv.position = p;
        }
    }

    let mut brep = BRep::new();

    // Add deduped vertices, recording their VertexId keyed by branch-set.
    let mut vid_by_branches: HashMap<Vec<u32>, VertexId> = HashMap::new();
    for dv in &dedup {
        let key: Vec<u32> = dv.branches.iter().map(|b| b.as_u32()).collect();
        let vid = brep.add_vertex(Vertex::new(dv.position, dv.branches.clone()));
        vid_by_branches.insert(key, vid);
    }

    // Group edge points by canonical signature. We iterate the groups in a
    // **sorted** order, not raw `HashMap` order: the std hasher's seed advances
    // between weaves in one process (on wasm it is a per-allocation counter, not
    // entropy), so iterating the map directly makes the whole extraction
    // order-dependent — and therefore erratic across regenerations. The emitted
    // `EdgeId` order (feeds idHash) and the `dangling` list (feeds reflex-vertex
    // reconstruction) would both drift run to run. Sorting pins them.
    let mut groups: HashMap<(u32, u32), Vec<EdgePoint>> = HashMap::new();
    for ep in &edge_points {
        let key = (ep.signature.0.as_u32(), ep.signature.1.as_u32());
        groups.entry(key).or_default().push(*ep);
    }
    let mut group_keys: Vec<(u32, u32)> = groups.keys().copied().collect();
    group_keys.sort_unstable();

    // Analytic branch→surface table, used to reject phantom signature groups
    // (DL-52): a real edge lies on both faces, a contributor-eps phantom does
    // not. Branches without an analytic entry (tessellated sketch sweeps) are
    // not validated and never dropped. Under a domain warp the analytic map
    // describes the un-warped primitive (it would wrongly reject every warped
    // edge), so the filter is disabled there.
    let halfspaces = if body_has_warp(body) {
        HashMap::new()
    } else {
        crate::brep::weaving::branch_halfspace_map(body)
    };

    // Dangling chain endpoints (no vertex snapped) recorded for the reflex-
    // vertex reconstruction post-pass (DL-60 / Task #25).
    let mut dangling: Vec<DanglingEnd> = Vec::new();

    for &(sa, sb) in &group_keys {
        let pts = &groups[&(sa, sb)];
        // Collapse coincident edge points (DL-17) and get the characteristic
        // spacing of the distinct survivors.
        let (pts, spacing) = dedup_points(pts);
        // Degenerate group (one distinct point) → nothing to chain.
        if spacing <= 0.0 {
            continue;
        }

        let branch_a = BranchId(sa);
        let branch_b = BranchId(sb);

        // Candidate end-vertices: deduped vertices whose branch-set ⊇ {a, b}.
        let candidates: Vec<(VertexId, [f64; 3])> = dedup
            .iter()
            .filter(|dv| dv.branches.contains(&branch_a) && dv.branches.contains(&branch_b))
            .map(|dv| {
                let key: Vec<u32> = dv.branches.iter().map(|b| b.as_u32()).collect();
                (vid_by_branches[&key], dv.position)
            })
            .collect();

        // General SDF edge tracer (DL-68): when both branches carry an analytic
        // surface, recover the seam directly from the two fields — project each
        // seed exactly onto `{f_a = 0 ∧ f_b = 0}` and march the tangent, bounded
        // by the candidate triple vertices. ONE method for the whole analytic
        // surface zoo (plane / sphere / cylinder / cone / torus and their
        // cross-pairs), resolution-independent, replacing DL-67's per-pair
        // closed-form circle and superseding the sampled line/circle fits for
        // analytic pairs. Falls through to those fits for curved sketch-sweep
        // walls (no analytic field) and under a domain warp (the analytic map
        // describes the un-warped primitive, so `halfspaces` is empty there).
        //
        // The tracer's bounding set is restricted to **real** junctions: a
        // coarse-octree contributor bleed can mint a phantom triple vertex on a
        // smooth seam (a non-touching face spuriously active in one cell), and
        // marching to it would chop the seam into arcs at a fake corner. Such a
        // vertex misses its bleed branch's surface by a feature size and is
        // dropped here (it stays in `candidates` for the legacy fits, which do
        // not snap to it unless an endpoint actually lands there).
        let bounding: Vec<[f64; 3]> = dedup
            .iter()
            .filter(|dv| dv.branches.contains(&branch_a) && dv.branches.contains(&branch_b))
            .filter(|dv| vertex_bounds_seam(&halfspaces, &dv.branches, dv.position, spacing))
            .map(|dv| dv.position)
            .collect();
        let pos: Vec<[f64; 3]> = pts.iter().map(|p| p.position).collect();
        let traced = match (branch_field(&halfspaces, sa), branch_field(&halfspaces, sb)) {
            (Some(fa), Some(fb)) => trace_seam_chains(&fa, &fb, &pos, &bounding, spacing),
            _ => None,
        };

        // Global per-group primitive fit (DL-51): a signature group from an
        // extrude / revolve body fragments into many gappy spatial runs even
        // though it is topologically ONE edge. Try to fit the whole group with
        // a single line, then a single circle; either collapses the fragments
        // into one clean oriented chain. Only if neither fits (a genuinely
        // multi-edge signature, DL-11) do we fall back to gap-chaining.
        let fit_tol = spacing * FIT_RESIDUAL_FACTOR;
        let chains: Vec<Chain> = if let Some(component_chains) = traced {
            component_chains
        } else if let Some(samples) = fit_line(&pts, fit_tol) {
            vec![Chain { samples, closed: None }]
        } else if let Some(samples) = fit_circle(&pts, fit_tol) {
            vec![Chain { samples, closed: None }]
        } else {
            let gap_threshold = spacing * CHAIN_GAP_FACTOR;
            chain_group(&pts, gap_threshold)
        };

        let snap_tol = spacing * SNAP_FACTOR;
        let loop_tol = spacing * LOOP_FACTOR;

        for chain in chains {
            if chain.samples.is_empty() {
                continue;
            }
            // Phantom-signature filter (DL-52): a consolidated chain survives
            // only if all its samples lie on *both* branches' analytic
            // surfaces. The membership tolerance is one sample spacing — loose
            // enough for on-surface jitter, far tighter than the cell-scale
            // offset a contributor-eps phantom pairing exhibits.
            if !chain_on_both_surfaces(&halfspaces, sa, sb, &chain.samples, spacing) {
                continue;
            }
            let first = chain.samples.first().unwrap().point;
            let last = chain.samples.last().unwrap().point;

            // Endpoint topology. The tracer already decided it (`chain.closed`):
            // a closed ring carries no vertices, an open arc snaps its two ends
            // to the real bounding vertices it stopped on. A sample-fit / chained
            // run (`closed = None`) goes through the legacy snap + loop heuristic.
            let (start, end, is_loop) = match chain.closed {
                Some(true) => (None, None, true),
                Some(false) => (
                    snap_to_vertex(first, &candidates, snap_tol),
                    snap_to_vertex(last, &candidates, snap_tol),
                    false,
                ),
                None => {
                    let start = snap_to_vertex(first, &candidates, snap_tol);
                    let end = snap_to_vertex(last, &candidates, snap_tol);
                    // Closed-loop detection: no vertex candidates and ends meet.
                    let is_loop = start.is_none()
                        && end.is_none()
                        && chain.samples.len() >= 3
                        && dist(first, last) <= loop_tol;
                    if is_loop {
                        (None, None, true)
                    } else {
                        (start, end, false)
                    }
                }
            };

            let eid = brep.add_edge(Edge::new(
                EdgeGeometry::Hermite(chain.samples),
                branch_a,
                branch_b,
                start,
                end,
            ));

            // Record endpoints that snapped to no vertex (and are not a closed
            // loop). At a reflex corner the missing Phase-3 vertex leaves the
            // three incident edges dangling here; the post-pass recovers it.
            if !is_loop {
                if start.is_none() {
                    dangling.push(DanglingEnd { edge: eid, at_start: true, pos: first, spacing });
                }
                if end.is_none() {
                    dangling.push(DanglingEnd { edge: eid, at_start: false, pos: last, spacing });
                }
            }
        }
    }

    reconcile_dangling_endpoints(body, &mut brep, &dangling);

    brep
}

/// A chain endpoint that snapped to no vertex — input to the reflex-vertex
/// reconstruction post-pass.
struct DanglingEnd {
    edge: EdgeId,
    /// `true` if this is the edge's `start` endpoint, `false` for `end`.
    at_start: bool,
    pos: [f64; 3],
    /// Characteristic sample spacing of the owning group (≈ the local octree
    /// resolution), used to scale the clustering tolerance and synthetic cell.
    spacing: f64,
}

/// Tolerance (as a multiple of local edge spacing) for matching a dangling end
/// to a vertex or another dangling end. The edge-walk's per-group line fit can
/// stop a few cells short of a corner (the corner region is a Vertex/Edge cell
/// the fit's points avoid), so the slack is a handful of cells.
const RECONCILE_TOL_FACTOR: f64 = 4.0;
/// Synthetic reconstruction-cell half-size as a multiple of local edge
/// spacing — wide enough that its corner probes reach into each incident
/// face's region.
const REFLEX_CELL_HALF_FACTOR: f32 = 3.0;

/// Reconcile every dangling edge endpoint with a topological vertex (DL-60).
///
/// `build_wireframe`'s per-group [`snap_to_vertex`] uses each group's local
/// spacing as its snap radius; an endpoint whose line-fit stopped a few cells
/// short of a *convex* corner can fall just outside it, and a *reflex* corner
/// has no extracted vertex to snap to at all (see below). Both leave the edge
/// with a `None` endpoint, which forces `order_loops` onto its unordered
/// fallback for every face that touches the gap. This pass closes both:
///
/// 1. **Re-snap to an existing vertex** — for each dangling end, the nearest
///    vertex whose branch-set ⊇ the edge's signature within a generous radius.
///    Branch-set containment (not distance alone) keeps the match topologically
///    correct, so this is a clean consolidation, not a fuzzy nearest-point grab.
///
/// 2. **Reconstruct missing reflex corners** — the octree only tags a `Vertex`
///    cell where ≥3 branches are simultaneously active at the apex. At a reflex
///    (concave) corner the solid opens past 180°, so the third face is active
///    only in a near-measure-zero sliver and the corner cell is mis-read as a
///    2-branch `Edge`; the corner never enters the vertex set. We recover it
///    from the topology that *was* detected: cluster the still-dangling ends and
///    where ≥3 edges spanning ≥3 distinct branches meet, re-solve the analytic
///    vertex from a synthetic `Vertex` cell there — reusing [`extract_vertex`]'s
///    plane-intersection + refinement + acceptance gates, so a cluster that is
///    not a real corner is rejected. Resolution-independent: the corner comes
///    from the analytic face planes, not from finer sampling.
fn reconcile_dangling_endpoints(body: &BoundedCsg, brep: &mut BRep, dangling: &[DanglingEnd]) {
    // ── Stage 1: re-snap to an existing branch-compatible vertex ──────────────
    let mut remaining: Vec<usize> = Vec::new();
    for (k, d) in dangling.iter().enumerate() {
        let (sa, sb) = brep.edges[d.edge.0 as usize].signature;
        let tol = d.spacing * RECONCILE_TOL_FACTOR;
        let mut best: Option<(VertexId, f64)> = None;
        for (vi, v) in brep.vertices.iter().enumerate() {
            if !(v.branches.contains(&sa) && v.branches.contains(&sb)) {
                continue;
            }
            let dd = dist(d.pos, v.position);
            if dd <= tol && best.is_none_or(|(_, bd)| dd < bd) {
                best = Some((VertexId(vi as u32), dd));
            }
        }
        match best {
            Some((vid, _)) => set_endpoint(brep, d, vid),
            None => remaining.push(k),
        }
    }

    // ── Stage 2: reconstruct missing (reflex) corners from clusters ───────────
    let mut used = vec![false; remaining.len()];
    for a in 0..remaining.len() {
        if used[a] {
            continue;
        }
        let seed = &dangling[remaining[a]];
        let tol = seed.spacing * RECONCILE_TOL_FACTOR;
        let members: Vec<usize> = (a..remaining.len())
            .filter(|&b| !used[b] && dist(seed.pos, dangling[remaining[b]].pos) <= tol)
            .collect();
        // A real corner joins ≥3 incident edges spanning ≥3 distinct branches.
        if members.len() < 3 {
            continue;
        }
        let mut branches: Vec<BranchId> = Vec::new();
        for &b in &members {
            let (sa, sb) = brep.edges[dangling[remaining[b]].edge.0 as usize].signature;
            for br in [sa, sb] {
                if !branches.contains(&br) {
                    branches.push(br);
                }
            }
        }
        if branches.len() < 3 {
            continue;
        }
        branches.sort_unstable_by_key(|b| b.as_u32());

        // Centroid seed + local scale for the synthetic cell.
        let mut sum = [0.0f64; 3];
        let mut scale = 0.0f64;
        for &b in &members {
            let p = dangling[remaining[b]].pos;
            sum[0] += p[0];
            sum[1] += p[1];
            sum[2] += p[2];
            scale = scale.max(dangling[remaining[b]].spacing);
        }
        let n = members.len() as f64;
        let center = [(sum[0] / n) as f32, (sum[1] / n) as f32, (sum[2] / n) as f32];
        let h = scale as f32 * REFLEX_CELL_HALF_FACTOR;

        let cell = OctreeCell {
            aabb: Aabb {
                min: [center[0] - h, center[1] - h, center[2] - h],
                max: [center[0] + h, center[1] + h, center[2] + h],
            },
            class: CellClass::Vertex,
            branches,
        };
        let Some(vertex) = extract_vertex(body, &cell) else {
            continue;
        };
        let vid = brep.add_vertex(vertex);
        for &b in &members {
            used[b] = true;
            set_endpoint(brep, &dangling[remaining[b]], vid);
        }
    }
}

/// Wire a dangling end's edge to `vid` on the recorded side.
fn set_endpoint(brep: &mut BRep, d: &DanglingEnd, vid: VertexId) {
    let e = &mut brep.edges[d.edge.0 as usize];
    if d.at_start {
        e.start = Some(vid);
    } else {
        e.end = Some(vid);
    }
}

/// Snap a chain endpoint to the nearest candidate vertex within `tol`.
fn snap_to_vertex(p: [f64; 3], candidates: &[(VertexId, [f64; 3])], tol: f64) -> Option<VertexId> {
    let mut best: Option<(VertexId, f64)> = None;
    for &(vid, vp) in candidates {
        let d = dist(p, vp);
        if d <= tol {
            best = match best {
                None => Some((vid, d)),
                Some((_, bd)) if d < bd => Some((vid, d)),
                other => other,
            };
        }
    }
    best.map(|(vid, _)| vid)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brep::octree::surface_gradient;
    use crate::csg::CsgNode;
    use fielddraft_protocol::{FeatureId, Primitive};
    use std::collections::HashSet;

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

    /// Index of the single varying axis of an edge chain (the axis whose span
    /// across the samples is the largest), plus the sign of the walk.
    fn varying_axis(samples: &[HermiteSample]) -> usize {
        let first = samples.first().unwrap().point;
        let last = samples.last().unwrap().point;
        let d = sub(last, first);
        let mut axis = 0;
        for a in 1..3 {
            if d[a].abs() > d[axis].abs() {
                axis = a;
            }
        }
        axis
    }

    #[test]
    fn cube_wireframe_has_12_edges_8_vertices() {
        let body = box_body([1.0, 1.0, 1.0]);
        let octree = Octree::build(&body, 0.25);
        let brep = build_wireframe(&body, &octree);

        assert_eq!(brep.vertex_count(), 8, "a cube has 8 corners");
        assert_eq!(brep.edge_count(), 12, "a cube has 12 edges");
        assert_eq!(brep.validate(), Ok(()), "wireframe must be well-formed");

        let mut signatures = HashSet::new();
        for e in &brep.edges {
            // Bounded by two distinct, in-range vertices (a cube has no loops).
            let s = e.start.expect("cube edge must have a start vertex");
            let en = e.end.expect("cube edge must have an end vertex");
            assert_ne!(s, en, "edge start and end must differ");
            assert!((s.0 as usize) < brep.vertex_count());
            assert!((en.0 as usize) < brep.vertex_count());

            // Distinct signatures.
            assert!(
                signatures.insert((e.signature.0.as_u32(), e.signature.1.as_u32())),
                "duplicate edge signature {:?}",
                e.signature
            );

            // Hermite geometry, on-surface and spatially monotone.
            let samples = match &e.geometry {
                EdgeGeometry::Hermite(s) => s,
                other => panic!("expected Hermite geometry, got {other:?}"),
            };
            assert!(samples.len() >= 2, "edge needs at least 2 samples");

            let axis = varying_axis(samples);
            let dir = samples.last().unwrap().point[axis] - samples.first().unwrap().point[axis];
            let sign = dir.signum();
            for w in samples.windows(2) {
                let step = (w[1].point[axis] - w[0].point[axis]) * sign;
                // Strictly increasing along the varying axis: no jumps back.
                assert!(
                    step > 0.0,
                    "edge samples not monotone along axis {axis}: {:?} -> {:?}",
                    w[0].point,
                    w[1].point
                );
            }

            for hs in samples {
                let p = [hs.point[0] as f32, hs.point[1] as f32, hs.point[2] as f32];
                let (f, _) = surface_gradient(&body, p);
                assert!(
                    f.abs() < 1e-2,
                    "edge sample off surface: f = {f}, p = {:?}",
                    hs.point
                );
            }
        }
        assert_eq!(signatures.len(), 12, "12 distinct cube edge signatures");

        // The 8 vertices sit at the (±1, ±1, ±1) corners.
        let mut corners = HashSet::new();
        for v in &brep.vertices {
            for c in v.position {
                assert!(
                    (c.abs() - 1.0).abs() < 5e-2,
                    "vertex not at a corner: {:?}",
                    v.position
                );
            }
            corners.insert((
                v.position[0].signum() as i32,
                v.position[1].signum() as i32,
                v.position[2].signum() as i32,
            ));
        }
        assert_eq!(corners.len(), 8, "expected 8 distinct corner positions");
    }

    #[test]
    fn difference_body_wireframe_on_surface() {
        // A box with a corner notch cut out. Every edge sample must lie on the
        // real surface, and every edge must carry oriented Hermite geometry.
        let outer = CsgNode::Leaf {
            id: FeatureId::from_u32(1),
            primitive: Primitive::Box {
                center: [0.0, 0.0, 0.0],
                half_extents: [1.0, 1.0, 1.0],
            },
        };
        let cutter = CsgNode::Leaf {
            id: FeatureId::from_u32(2),
            primitive: Primitive::Box {
                center: [0.6, 0.6, 0.0],
                half_extents: [0.6, 0.6, 2.0],
            },
        };
        let csg = CsgNode::Difference(Box::new(outer), Box::new(cutter));
        let body = BoundedCsg::from_csg(&csg);
        let octree = Octree::build(&body, 0.25);
        let brep = build_wireframe(&body, &octree);

        assert_eq!(brep.validate(), Ok(()));
        assert!(brep.edge_count() > 0, "the notched box has surface edges");
        assert!(
            brep.vertex_count() > 0,
            "the notched box has corner vertices"
        );

        for e in &brep.edges {
            let samples = match &e.geometry {
                EdgeGeometry::Hermite(s) => s,
                other => panic!("expected Hermite geometry, got {other:?}"),
            };
            for hs in samples {
                let p = [hs.point[0] as f32, hs.point[1] as f32, hs.point[2] as f32];
                let (f, _) = surface_gradient(&body, p);
                assert!(
                    f.abs() < 5e-2,
                    "edge sample off surface: f = {f}, p = {:?}",
                    hs.point
                );
            }
        }
    }

    #[test]
    fn vertex_dedup_collapses_branch_set_to_centroid() {
        // Three vertex cells sharing one branch-triple collapse to one vertex
        // at the centroid of their positions.
        let b = vec![BranchId(1), BranchId(2), BranchId(3)];
        let raw = vec![
            Vertex::new([0.0, 0.0, 0.0], b.clone()),
            Vertex::new([2.0, 0.0, 0.0], b.clone()),
            Vertex::new([1.0, 3.0, 0.0], b.clone()),
            // A different triple → its own vertex.
            Vertex::new([5.0, 5.0, 5.0], vec![BranchId(1), BranchId(2), BranchId(4)]),
        ];
        let dedup = dedup_vertices(&raw);
        assert_eq!(dedup.len(), 2);
        let triple = dedup
            .iter()
            .find(|d| d.branches == b)
            .expect("the shared triple must survive");
        assert!((triple.position[0] - 1.0).abs() < 1e-9);
        assert!((triple.position[1] - 1.0).abs() < 1e-9);
        assert!((triple.position[2] - 0.0).abs() < 1e-9);
    }

    #[test]
    fn chain_group_splits_on_large_gap() {
        // Two disjoint runs of collinear points sharing a signature: a tight
        // cluster near the origin and another far away. With a gap threshold
        // between the intra-cluster spacing and the inter-cluster gap, the
        // walk must produce two chains.
        let sig = (BranchId(1), BranchId(2));
        let mk = |x: f64| EdgePoint {
            position: [x, 0.0, 0.0],
            tangent: [1.0, 0.0, 0.0],
            signature: sig,
        };
        let pts = vec![mk(0.0), mk(1.0), mk(2.0), mk(20.0), mk(21.0), mk(22.0)];
        // Median spacing ≈ 1.0 → threshold 3.0; the 18-unit jump splits.
        let chains = chain_group(&pts, 1.0 * CHAIN_GAP_FACTOR);
        assert_eq!(chains.len(), 2, "expected two disjoint chains");
        for c in &chains {
            assert_eq!(c.samples.len(), 3);
            // Oriented monotone along +x.
            for w in c.samples.windows(2) {
                assert!(w[1].point[0] > w[0].point[0]);
                assert!(w[0].tangent[0] > 0.0, "tangent should point forward");
            }
        }
    }

    // ── DL-51: global per-group primitive fits ───────────────────────────

    #[test]
    fn fit_line_consolidates_fragmented_collinear_points() {
        // Points spread along +x in a SCRAMBLED order with a wide gap in the
        // middle (the fragmentation `chain_group` would split on). A single
        // line fits → one ordered chain spanning all of them, monotone in x.
        let sig = (BranchId(1), BranchId(2));
        let mk = |x: f64, jitter: f64| EdgePoint {
            position: [x, jitter, 0.0],
            tangent: [1.0, 0.0, 0.0],
            signature: sig,
        };
        // Out of order, with a gap from 3→9 (would fragment under gap-chaining).
        let pts = vec![
            mk(9.0, 0.01),
            mk(1.0, -0.01),
            mk(10.0, 0.0),
            mk(0.0, 0.0),
            mk(3.0, 0.01),
            mk(2.0, -0.01),
        ];
        // tol generous vs the 0.01 perpendicular jitter.
        let samples = fit_line(&pts, 0.1).expect("collinear points must fit a line");
        assert_eq!(samples.len(), 6, "all points kept in one chain");
        for w in samples.windows(2) {
            assert!(
                w[1].point[0] > w[0].point[0],
                "samples sorted monotone along the line direction"
            );
        }
    }

    #[test]
    fn fit_line_rejects_two_parallel_edges() {
        // Two disjoint parallel runs (DL-11): they cannot lie on ONE line, so
        // the perpendicular residual blows past `tol` and the fit fails →
        // build_wireframe falls back to gap-chaining.
        let sig = (BranchId(1), BranchId(2));
        let mk = |x: f64, y: f64| EdgePoint {
            position: [x, y, 0.0],
            tangent: [1.0, 0.0, 0.0],
            signature: sig,
        };
        let pts = vec![
            mk(0.0, 0.0),
            mk(1.0, 0.0),
            mk(2.0, 0.0),
            mk(0.0, 5.0),
            mk(1.0, 5.0),
            mk(2.0, 5.0),
        ];
        assert!(
            fit_line(&pts, 0.5).is_none(),
            "two parallel edges must NOT fit a single line"
        );
    }

    #[test]
    fn fit_circle_consolidates_co_circular_points() {
        // 16 points on a unit circle in the z=0 plane (scrambled). A single
        // circle fits → one closed chain sorted by angle, no duplicated wrap
        // sample.
        let sig = (BranchId(1), BranchId(2));
        let n = 16;
        let mut pts: Vec<EdgePoint> = (0..n)
            .map(|i| {
                let a = std::f64::consts::TAU * (i as f64) / (n as f64);
                EdgePoint {
                    position: [a.cos(), a.sin(), 0.0],
                    tangent: [-a.sin(), a.cos(), 0.0],
                    signature: sig,
                }
            })
            .collect();
        // Scramble.
        pts.swap(0, 9);
        pts.swap(3, 12);
        let samples = fit_circle(&pts, 0.05).expect("co-circular points must fit a circle");
        assert_eq!(samples.len(), n, "one sample per point, no wrap duplicate");
        // Sorted by angle: consecutive samples are one angular step apart, so
        // every neighbouring chord equals the regular polygon edge length
        // (~2·sin(π/n)); the single wrap step from last→first matches too. This
        // is the closed-loop ordering the weaver needs.
        let step = 2.0 * (std::f64::consts::PI / n as f64).sin();
        for k in 0..n {
            let a = samples[k].point;
            let b = samples[(k + 1) % n].point;
            assert!(
                (dist(a, b) - step).abs() < 1e-6,
                "neighbouring samples not one angular step apart: {a:?} -> {b:?}"
            );
        }
    }

    #[test]
    fn fit_circle_rejects_a_straight_line() {
        // Collinear points are degenerate for a circle fit (infinite radius /
        // non-planar-spread fails) → None, so a straight edge stays a line.
        let sig = (BranchId(1), BranchId(2));
        let pts: Vec<EdgePoint> = (0..10)
            .map(|i| EdgePoint {
                position: [i as f64, 0.0, 0.0],
                tangent: [1.0, 0.0, 0.0],
                signature: sig,
            })
            .collect();
        assert!(
            fit_circle(&pts, 0.05).is_none(),
            "a straight line is not a circle"
        );
    }
}

