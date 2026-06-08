//! B-Rep extraction, **Phase 3: edge & vertex extraction + phantom filter**.
//!
//! Implements the analytic feature extraction described in
//! `docs/GEOMETRY-KERNEL-CONCEPT.md` ("Phase 3: Kanten- und
//! Vertex-Extraktion"). Phase 1 ([`crate::brep::octree`]) hands us
//! classified leaf cells; this phase turns each
//!
//! - **`Edge` cell** (exactly 2 colliding branches) → an [`EdgePoint`]: a
//!   point on the intersection line of the two branch tangent planes, plus
//!   the edge tangent and the canonical branch signature, and
//! - **`Vertex` cell** (≥3 colliding branches) → a [`crate::brep::Vertex`]:
//!   the least-squares intersection point of all branch tangent planes.
//!
//! Each candidate is run through a **phantom filter** (concept lines
//! 235–241): the resolved point is re-evaluated against the field and kept
//! only if it lies on the surface (`|f| < ε_phantom`); medial-axis
//! (`f ≪ 0`) and external Voronoi-trench (`f ≫ 0`) candidates are dropped.
//!
//! This phase does **not** chain points into edges or fit splines — that is
//! C.4 (edge walker) and C.5 (spline fitting).
//!
//! ## Per-branch sample points (design decision, see DL-4)
//!
//! To get a clean tangent plane for each colliding branch we need a start
//! point that lies unambiguously in that branch's Voronoi region — *not* on
//! the fault, where two branches are simultaneously active and the normal is
//! ambiguous. We sample the cell's 8 corners + centre with
//! [`crate::csg::EvalMode::Logical`]; a sample "belongs" to branch `X` when
//! its contributor slice contains `X`. Per branch we pick the candidate
//! whose contributor slice contains `X` with the **smallest `|f|`** (closest
//! to the surface) and prefer samples where `X` is the *sole* contributor
//! (cleanest tangent), then project it via Phase 2. A branch with no clean
//! start point makes the whole cell non-extractable (we emit nothing — never
//! a phantom).

use std::collections::HashMap;

use nalgebra::{Matrix3, Vector3};

use crate::brep::octree::{surface_gradient, CellClass, Octree, OctreeCell};
use crate::brep::projection::{project_to_surface, ProjectedPoint};
use crate::brep::Vertex;
use crate::bvh::BoundedCsg;
use crate::csg::{grad_halfspace, sdf_halfspace, BranchId, EvalMode};
use crate::{quat_rotate, world_to_local};
use fielddraft_protocol::{HalfSpace, Primitive, Transform};

/// A branch → (analytic surface, local→world placement) table, as built by
/// [`crate::brep::weaving::branch_halfspace_map`]. Threaded into feature
/// extraction so a branch with an analytic surface is projected onto its OWN
/// surface (DL-75) instead of the body SDF — see [`project_branch_analytic`].
type HalfspaceMap = HashMap<u32, (HalfSpace, Transform)>;

/// Project `p` (world) exactly onto a branch's own analytic surface, given its
/// [`HalfSpace`] and local→world placement (DL-75).
///
/// The generic [`project_to_surface`] Newton-marches the *body* SDF, which for a
/// shelled body has two parallel zero-surfaces a wall-thickness apart (inner
/// wall + outer surface); a seed can slide from one onto the other, yielding a
/// tangent plane on the wrong sheet. Targeting the branch's own half-space (a
/// true Euclidean distance with unit gradient) removes that ambiguity: Newton in
/// the surface's local frame converges in one step for a plane and a few for a
/// quadric, then the point and its normal are mapped back to world.
fn project_branch_analytic(hs: &HalfSpace, xf: &Transform, p: [f32; 3]) -> Option<ProjectedPoint> {
    let mut lp = world_to_local(xf, p);
    let mut g = [0.0f32; 3];
    let mut converged = false;
    for _ in 0..PROJECT_MAX_ITER {
        let f = sdf_halfspace(hs, lp);
        g = grad_halfspace(hs, lp);
        let g2 = g[0] * g[0] + g[1] * g[1] + g[2] * g[2];
        if g2 < 1e-12 {
            return None;
        }
        if f.abs() < PROJECT_EPS {
            converged = true;
            break;
        }
        let s = f / g2;
        lp = [lp[0] - s * g[0], lp[1] - s * g[1], lp[2] - s * g[2]];
    }
    if !converged && sdf_halfspace(hs, lp).abs() >= PROJECT_EPS {
        return None;
    }
    // local → world position: scale, rotate, translate.
    let scaled = [lp[0] * xf.scale[0], lp[1] * xf.scale[1], lp[2] * xf.scale[2]];
    let rot = quat_rotate(xf.rotation, scaled);
    let position = [
        rot[0] + xf.translate[0],
        rot[1] + xf.translate[1],
        rot[2] + xf.translate[2],
    ];
    // local normal → world (inverse-transpose; for a rigid+scale placement this
    // is rotate(n / scale), then renormalise).
    let nl = [g[0] / xf.scale[0], g[1] / xf.scale[1], g[2] / xf.scale[2]];
    let nw = quat_rotate(xf.rotation, nl);
    let nlen = (nw[0] * nw[0] + nw[1] * nw[1] + nw[2] * nw[2]).sqrt();
    if nlen < 1e-9 {
        return None;
    }
    Some(ProjectedPoint {
        position,
        normal: [nw[0] / nlen, nw[1] / nlen, nw[2] / nlen],
    })
}

/// Does `body` contain a [`BoundedCsg::Shell`]? Only then is the analytic
/// branch-projection path engaged (DL-75); every other body keeps the exact
/// generic projection it had before, so non-shell weaves are byte-identical.
pub(crate) fn body_contains_shell(body: &BoundedCsg) -> bool {
    match body {
        BoundedCsg::Shell { .. } => true,
        BoundedCsg::Leaf { .. } | BoundedCsg::Empty => false,
        BoundedCsg::Union { left, right, .. }
        | BoundedCsg::Difference { left, right, .. }
        | BoundedCsg::Intersection { left, right, .. }
        | BoundedCsg::SmoothUnion { left, right, .. }
        | BoundedCsg::SmoothDifference { left, right, .. }
        | BoundedCsg::MaskedBlend { left, right, .. }
        | BoundedCsg::Morph { left, right, .. } => {
            body_contains_shell(left) || body_contains_shell(right)
        }
        BoundedCsg::Twist { inner, .. }
        | BoundedCsg::Bend { inner, .. }
        | BoundedCsg::Offset { inner, .. }
        | BoundedCsg::Displace { inner, .. } => body_contains_shell(inner),
        BoundedCsg::Extrude { .. } | BoundedCsg::Revolve { .. } => false,
    }
}

/// Surface-distance tolerance for the Newton projection (Phase 2). Tight —
/// this is the convergence criterion of the projection itself, not the
/// phantom test.
const PROJECT_EPS: f32 = 1e-6;

/// Iteration cap for the Newton projection. Euclidean fields converge in one
/// step, warped fields in 2–4; 16 is a generous hang-guard (see DL-8).
const PROJECT_MAX_ITER: usize = 16;

/// Tangent-magnitude floor below which `n_a × n_b` is treated as degenerate
/// (parallel / tangential normals → no well-defined edge direction).
const TINY_TANGENT: f32 = 1e-6;

/// Determinant / singular-value floor for the 3×3 plane-intersection solves.
const TINY_DET: f32 = 1e-9;

/// Factor relating the phantom tolerance to the cell diagonal: a candidate
/// survives when `|f(p)| < PHANTOM_EPS_FACTOR · diag`. See DL-5 for why the
/// phantom tolerance is cell-relative (not the tight `PROJECT_EPS`).
const PHANTOM_EPS_FACTOR: f32 = 0.25;

/// Cell-relative tolerance for selecting a **per-branch start seed** (see
/// [`project_branch`] / [`reproject_branch_near`]). Deliberately much tighter
/// than the classification/gate band: a seed must lie in the branch's *own*
/// Voronoi region (the branch is the single strictly-nearest surface) so its
/// projected tangent plane is unambiguous. The wide classification band
/// (`OctreeConfig::eps_factor`, and the sketch contributor band from DL-65)
/// intentionally reports *both* branches near a fault so the cell is found and
/// the gate can fire — but feeding that wide band to seed-finding would leave
/// every near-corner probe a shared contributor, collapsing the seed onto the
/// degenerate fault point where the field gradient is ill-defined and the
/// Newton projection diverges. Tightening the seed eps restores a clean
/// sole-contributor seed on each side of the edge (DL-65).
const SEED_EPS_FACTOR: f32 = 0.05;

/// Cell-relative tolerance for the *both-branches-active* gate (see
/// [`both_branches_active`], DL-63): a resolved edge point keeps its cell only
/// if, re-evaluating the hard min there, the second branch's field is within
/// `BOTH_ACTIVE_EPS_FACTOR · diag` of the live one. Tight enough to reject a
/// phantom seam that has slid onto one branch's free surface (the dormant
/// branch is then a whole feature-size away), loose enough to admit the f32
/// tie at a genuine intersection curve.
const BOTH_ACTIVE_EPS_FACTOR: f32 = 0.25;

/// Outer-refinement iteration cap for warped (curved) faces (see DL-38). The
/// initial one-shot linear/LSQ solve only first-order-approximates a curved
/// edge/vertex because the tangent planes are taken away from the true
/// feature; each refinement step re-anchors those planes near the current
/// estimate and re-solves. Euclidean (planar) faces converge in 0 extra
/// steps; warped fields in 2–4. The cap is a hang-guard.
const REFINE_MAX_ITER: usize = 8;

/// Convergence criterion for the outer refinement: the estimate is accepted
/// once it lies on every contributing surface to within this absolute
/// surface-distance tolerance (logical field value `|f|`). Looser than the
/// tight per-branch `PROJECT_EPS` because the linear solve places a point on
/// the *intersection of tangent planes*, which only equals the curved edge in
/// the limit; this is the residual we drive the outer loop against.
const REFINE_EPS: f32 = 1e-4;

/// Hard acceptance gate on the final surface distance of an extracted
/// edge/vertex point (see DL-38). The tangent-plane intersection is a
/// first-order construction, so on a strongly-curved (warped) feature the
/// refined residual can stall above `REFINE_EPS` for some cells. Rather than
/// emit an off-surface point we **drop** any cell whose final residual exceeds
/// this gate. Each topological edge/vertex is covered by many octree cells, so
/// dropping the worst ones still leaves clean covers (12 edges / 8 vertices)
/// while guaranteeing every returned point lies on the warped surface. The
/// value is well inside the phantom tolerance and tight enough for downstream
/// edge-walking / weaving.
const ACCEPT_EPS: f32 = 1e-3;

/// One extracted edge sample: a point on the surface edge, the edge tangent
/// there, and the canonical `(BranchId, BranchId)` signature of the two
/// colliding branches. The raw Phase-3 product for one `Edge` cell, in `f64`
/// to match the B-Rep geometry types. NOT yet chained or fitted (C.4/C.5).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EdgePoint {
    pub position: [f64; 3],
    pub tangent: [f64; 3],
    /// The two colliding branches, canonically sorted (`.0 <= .1`).
    pub signature: (BranchId, BranchId),
}

// ─── per-branch sampling ──────────────────────────────────────────────────

/// The 8 corners + centre of a cell, as `[f32; 3]` probe points.
fn probe_points(cell: &OctreeCell) -> [[f32; 3]; 9] {
    let a = cell.aabb;
    let mut pts = [[0.0f32; 3]; 9];
    for (i, slot) in pts.iter_mut().enumerate().take(8) {
        let x = if i & 1 == 0 { a.min[0] } else { a.max[0] };
        let y = if i & 2 == 0 { a.min[1] } else { a.max[1] };
        let z = if i & 4 == 0 { a.min[2] } else { a.max[2] };
        *slot = [x, y, z];
    }
    pts[8] = cell.center();
    pts
}

/// Find a clean start point for `branch` inside `cell` and project it.
///
/// A probe point is a candidate for `branch` iff its `Logical` contributor
/// slice contains `branch`. Among candidates we prefer those where `branch`
/// is the *sole* contributor (cleanest tangent, away from the fault); within
/// each preference class we take the smallest `|f|` (closest to the surface,
/// best Newton seed). Returns the projected `(point, normal)` or `None` if no
/// candidate exists or the projection diverges.
fn project_branch(
    body: &BoundedCsg,
    cell: &OctreeCell,
    branch: BranchId,
    halfspaces: &HalfspaceMap,
) -> Option<ProjectedPoint> {
    // Analytic fast path (DL-75): when the branch carries an analytic surface,
    // project the cell centre straight onto THAT surface. Only populated for
    // shelled bodies, whose two parallel zero-surfaces defeat the generic body-
    // SDF Newton below (a seed slides between the inner wall and the outer
    // surface). For every other body `halfspaces` is empty → unchanged.
    if let Some((hs, xf)) = halfspaces.get(&branch.as_u32()) {
        if let Some(proj) = project_branch_analytic(hs, xf, cell.center()) {
            return Some(proj);
        }
    }

    let diag = {
        let s = cell.aabb.size();
        (s[0] * s[0] + s[1] * s[1] + s[2] * s[2]).sqrt()
    };

    // Seed search (DL-65). The default is the original *wide* band (matching the
    // octree's classification eps), which the analytic seam extraction and the
    // edge-walker fitting are calibrated against. Project from it first; if that
    // succeeds we return the exact same result as before — every analytic body
    // is byte-for-byte unchanged.
    //
    // The wide band only breaks at a *perpendicular sweep corner*: there every
    // near-corner probe is a shared contributor (the sketch DL-65 band reports
    // both adjoining walls), so the best seed collapses onto the degenerate
    // fault point (`|f| ≈ 0`, ambiguous gradient) and the Newton projection
    // diverges. *Only then* do we rescue with a tight band, which exposes a
    // clean sole seed on the branch's own side of the corner. Because we gate
    // the rescue on the wide projection actually failing, a near-tangential
    // analytic seam (whose wide seed projects fine) never takes this path.
    // Seed band strategy (DL-65). Pure analytic hard CSG keeps the original
    // *wide* band: its half-space faces have clean sole-contributor regions, and
    // the seam extraction + edge-walker fitting are calibrated against those
    // seeds (touching them refragments a near-tangential sphere seam). A sweep or
    // domain warp instead has a degenerate fault at every corner — the DL-65
    // sketch band (or the warped field) reports both adjoining branches across a
    // wide neighbourhood, so the wide best seed collapses onto the fault point
    // (`|f| ≈ 0`, ambiguous gradient) and projects onto a neighbouring wall or
    // diverges. There we go *tight first* to land a clean sole seed on the
    // branch's own side, falling back to wide only if tight finds nothing.
    let (tight_eps, wide_eps) = (diag * SEED_EPS_FACTOR, diag * 0.5);
    let order = if body_prefers_wide_seed(body) {
        [wide_eps, tight_eps]
    } else {
        [tight_eps, wide_eps]
    };
    // Cross-sheet guard is only needed for shelled bodies (non-empty `halfspaces`
    // ⟺ a shell, since the map is shell-gated by `extract_features`). There a
    // body-SDF Newton can slide off `branch`'s sheet onto the parallel one
    // (inner wall vs outer surface a wall-thickness apart, DL-75), so the
    // projection is kept only if `branch` is still active at it. Off for every
    // other body, so non-shell weaves stay byte-identical.
    let shelled = !halfspaces.is_empty();
    for eps in order {
        if let Some((p, _)) = find_seed(body, cell, branch, eps) {
            if let Some(proj) = project_to_surface(body, p, PROJECT_EPS, PROJECT_MAX_ITER) {
                if shelled {
                    let (_, c) =
                        body.evaluate(proj.position, diag * SEED_EPS_FACTOR, EvalMode::Logical);
                    if !c.as_slice().contains(&branch) {
                        continue;
                    }
                }
                return Some(proj);
            }
        }
    }
    None
}

/// Re-seed every branch of a `Vertex` cell from an outward sole-contributor
/// probe and project it. Used only as the convex-corner rescue (DL-78) when the
/// in-cell projections collapse two faces onto one plane (rank-deficient vertex
/// solve — the original Box ∪ curved-partner bug). Returns the projected planes
/// that landed cleanly on their own branch (validated), which the caller re-solves.
fn reproject_branches_outward(
    body: &BoundedCsg,
    cell: &OctreeCell,
    diag: f32,
) -> Vec<(BranchId, ProjectedPoint)> {
    let validate_eps = diag * SEED_EPS_FACTOR;
    let mut planes = Vec::with_capacity(cell.branches.len());
    for &branch in &cell.branches {
        if let Some(seed) = find_outward_seed(body, cell, branch, diag) {
            if let Some(proj) = project_to_surface(body, seed, PROJECT_EPS, PROJECT_MAX_ITER) {
                if branch_active_at(body, proj.position, branch, validate_eps) {
                    planes.push((branch, proj));
                }
            }
        }
    }
    planes
}

/// Is `body` a clean hard-CSG combination of analytic primitives? Only those
/// get the original *wide* seed band in [`project_branch`]: their half-space
/// faces have clean sole-contributor Voronoi regions and the seam extraction is
/// calibrated against those seeds. Sketch sweeps and domain warps have a
/// degenerate fault at every corner (DL-65), so they seed tight-first instead.
///
/// NB: unlike the deleted `phantom_gate_eligible`, this no longer gates the
/// both-branches filter — that gate is universal now — it only picks the
/// numerical seed-search order.
fn body_prefers_wide_seed(body: &BoundedCsg) -> bool {
    match body {
        BoundedCsg::Empty => true,
        BoundedCsg::Leaf { primitive, .. } => matches!(
            primitive,
            Primitive::Box { .. }
                | Primitive::Sphere { .. }
                | Primitive::Cylinder { .. }
                | Primitive::Cone { .. }
                | Primitive::Torus { .. }
                | Primitive::Ellipsoid { .. }
                | Primitive::Capsule { .. }
                | Primitive::HexPrism { .. }
        ),
        BoundedCsg::Union { left, right, .. }
        | BoundedCsg::Difference { left, right, .. }
        | BoundedCsg::Intersection { left, right, .. }
        | BoundedCsg::SmoothUnion { left, right, .. }
        | BoundedCsg::SmoothDifference { left, right, .. }
        | BoundedCsg::MaskedBlend { left, right, .. }
        | BoundedCsg::Morph { left, right, .. } => {
            body_prefers_wide_seed(left) && body_prefers_wide_seed(right)
        }
        // An offset inherits its carrier's field character: offset-of-analytic
        // stays an exact Euclidean field (wide seed OK), offset-of-sweep/warp
        // stays tight. Recursing into the carrier picks the right one.
        BoundedCsg::Shell { inner, .. } | BoundedCsg::Offset { inner, .. } => {
            body_prefers_wide_seed(inner)
        }
        BoundedCsg::Twist { .. }
        | BoundedCsg::Bend { .. }
        | BoundedCsg::Displace { .. }
        | BoundedCsg::Extrude { .. }
        | BoundedCsg::Revolve { .. } => false,
    }
}

/// Find the best per-branch start probe within the given contributor `eps`:
/// among the cell's 9 probe points whose `Logical` contributor slice contains
/// `branch`, prefer those where `branch` is the *sole* contributor (cleanest
/// tangent, away from the fault); within each class take the smallest `|f|`
/// (closest to the surface, best Newton seed). Returns the probe and whether
/// `branch` was its sole contributor, or `None` if no probe reports `branch`.
fn find_seed(
    body: &BoundedCsg,
    cell: &OctreeCell,
    branch: BranchId,
    eps: f32,
) -> Option<([f32; 3], bool)> {
    let mut best: Option<([f32; 3], bool, f32)> = None; // (point, sole, |f|)
    for p in probe_points(cell) {
        let (f, contributors) = body.evaluate(p, eps, EvalMode::Logical);
        let slice = contributors.as_slice();
        if !slice.contains(&branch) {
            continue;
        }
        let sole = slice.len() == 1;
        let fabs = f.abs();
        let better = match best {
            None => true,
            // Sole-contributor candidates beat shared ones outright.
            Some((_, best_sole, best_fabs)) => match (sole, best_sole) {
                (true, false) => true,
                (false, true) => false,
                _ => fabs < best_fabs,
            },
        };
        if better {
            best = Some((p, sole, fabs));
        }
    }
    best.map(|(p, sole, _)| (p, sole))
}

/// True iff `branch` is among the (tight-eps) active contributors at `p` — i.e.
/// the Newton projection actually landed on `branch`'s own surface rather than
/// sliding onto a neighbouring face at a convex corner.
fn branch_active_at(body: &BoundedCsg, p: [f32; 3], branch: BranchId, eps: f32) -> bool {
    let (_, c) = body.evaluate(p, eps, EvalMode::Logical);
    c.as_slice().contains(&branch)
}

/// Convex-corner rescue seed (DL-78, generic — no analytic half-spaces).
///
/// At a `Vertex` cell every in-cell probe sits on the multi-branch fault, so a
/// Newton projection for a *side* branch slides onto an adjacent face and two
/// tangent planes collapse (rank-deficient vertex solve — the original
/// Box ∪ curved-partner bug). We fan outward from the cell centre along the 26
/// cube directions at geometrically growing radii and return the closest point
/// where `branch` is the **sole** active contributor — a clean point on
/// `branch`'s own surface, away from the fault. A planar face's sole region is
/// reached at ≈1 cell diagonal along a face-diagonal direction; growing radii
/// cover thin slivers and curved faces whose sole region sits further out.
fn find_outward_seed(
    body: &BoundedCsg,
    cell: &OctreeCell,
    branch: BranchId,
    diag: f32,
) -> Option<[f32; 3]> {
    let c = cell.center();
    let eps = (diag * SEED_EPS_FACTOR).max(1e-5);
    for &k in &[1.0f32, 2.0, 4.0, 8.0] {
        let r = diag * k;
        // Among the sole-`branch` hits at this radius, keep the one whose
        // direction is least diagonal (smallest ‖dir‖) — it stays nearest the
        // face, giving the best-anchored tangent plane.
        let mut best: Option<([f32; 3], f32)> = None;
        for dx in [-1.0f32, 0.0, 1.0] {
            for dy in [-1.0f32, 0.0, 1.0] {
                for dz in [-1.0f32, 0.0, 1.0] {
                    if dx == 0.0 && dy == 0.0 && dz == 0.0 {
                        continue;
                    }
                    let len = (dx * dx + dy * dy + dz * dz).sqrt();
                    let p = [
                        c[0] + dx / len * r,
                        c[1] + dy / len * r,
                        c[2] + dz / len * r,
                    ];
                    let (_, contributors) = body.evaluate(p, eps, EvalMode::Logical);
                    let slice = contributors.as_slice();
                    if slice.len() == 1 && slice[0] == branch && best.map_or(true, |(_, bl)| len < bl)
                    {
                        best = Some((p, len));
                    }
                }
            }
        }
        if let Some((p, _)) = best {
            return Some(p);
        }
    }
    None
}

/// Re-project `branch` from a seed *near* `p`, nudged into `branch`'s Voronoi
/// region, so the resulting tangent plane is anchored at the true feature
/// rather than at a far cell corner (see DL-38).
///
/// `p` is the current edge/vertex estimate (on or very near the fault between
/// the colliding branches). Each direction in `dirs` lies in `branch`'s
/// tangent plane and points away from the fault, so stepping a small `delta`
/// along ±`dir` lands inside `branch`'s region. We probe ±each direction,
/// keep the seed where `branch` actually contributes (prefer sole-contributor,
/// then smallest `|f|`), and Newton-project from it.
///
/// Returns `None` if no nudge lands in the branch's region or the projection
/// diverges — the caller then keeps the previous estimate.
fn reproject_branch_near(
    body: &BoundedCsg,
    branch: BranchId,
    p: [f32; 3],
    dirs: &[Vector3<f32>],
    delta: f32,
    halfspaces: &HalfspaceMap,
) -> Option<ProjectedPoint> {
    // Analytic fast path (DL-75): re-anchor straight onto the branch's own
    // surface, never the wrong sheet of a shelled body's twin surfaces.
    if let Some((hs, xf)) = halfspaces.get(&branch.as_u32()) {
        if let Some(proj) = project_branch_analytic(hs, xf, p) {
            return Some(proj);
        }
    }
    // Tight membership band (caller passes `delta = diag * 0.25`, so this is
    // ≈ `diag * SEED_EPS_FACTOR`): the nudged seed must land in the branch's
    // own region, not on the shared DL-65 fault band of a warped sweep/corner,
    // else the re-anchored tangent plane is taken at the fault and refinement
    // stalls. See SEED_EPS_FACTOR.
    let eps = (delta * (SEED_EPS_FACTOR * 4.0)).max(1e-5);
    let mut best_seed: Option<([f32; 3], bool, f32)> = None;
    for dir in dirs {
        let dir_len = dir.norm();
        if dir_len < TINY_TANGENT {
            continue;
        }
        let step = dir * (delta / dir_len);
        for sign in [1.0f32, -1.0] {
            let seed = [
                p[0] + sign * step[0],
                p[1] + sign * step[1],
                p[2] + sign * step[2],
            ];
            let (f, contributors) = body.evaluate(seed, eps, EvalMode::Logical);
            let slice = contributors.as_slice();
            if !slice.contains(&branch) {
                continue;
            }
            let sole = slice.len() == 1;
            let fabs = f.abs();
            let better = match best_seed {
                None => true,
                Some((_, bsole, bf)) => match (sole, bsole) {
                    (true, false) => true,
                    (false, true) => false,
                    _ => fabs < bf,
                },
            };
            if better {
                best_seed = Some((seed, sole, fabs));
            }
        }
    }

    // No nudge landed in the branch's region (very thin region / huge
    // curvature): project `p` itself as a last resort.
    let seed = best_seed.map(|(s, _, _)| s).unwrap_or(p);
    project_to_surface(body, seed, PROJECT_EPS, PROJECT_MAX_ITER)
}

// ─── phantom filter ─────────────────────────────────────────────────────────

/// Phantom filter: keep a candidate point only if it lies on the surface.
/// `|f(p)| < eps_phantom` → real surface feature; `f ≪ 0` (medial axis) and
/// `f ≫ 0` (external Voronoi trench) are rejected. `eps_phantom` is chosen
/// cell-relative by the callers (see DL-5).
fn passes_phantom_filter(body: &BoundedCsg, p: [f32; 3], eps_phantom: f32) -> bool {
    let (f, _) = surface_gradient(body, p);
    f.abs() < eps_phantom
}

/// True iff **both** signature branches are simultaneously active (within
/// `eps` of the hard-min surface) at `p` — the defining condition of a *real*
/// edge: the intersection curve of two branch zero-sets.
///
/// The `edge_residual`/`passes_phantom_filter` gates only check the *union*
/// field `|f(p)|`, which a point on **one** branch's free surface satisfies
/// trivially. A phantom edge cell forms wherever a second branch is merely the
/// runner-up across the cell corners (e.g. a box-face half-space alongside a
/// sphere that pokes out near that face) without that branch ever reaching the
/// zero-set: the tangent-plane solve then slides the point along the live
/// branch, *off* the dormant one, producing a seam that curls onto the free
/// surface (DL-63). Re-evaluating the hard min at `p` returns only the live
/// branch as a contributor there, so requiring both branches in the
/// contributor set rejects the phantom while keeping every genuine seam point
/// (where the two surfaces truly tie at zero).
///
/// Since DL-65 this gate is **universal** — applied to every body. The earlier
/// allowlist (`phantom_gate_eligible`) excluded sketch sweeps and domain warps
/// only because their `evaluate` reported a single contributor at a genuine
/// edge, so the gate would have dropped real edges. That was a representational
/// artifact, not geometry: sweeps now emit eps-banded multi-contributors
/// ([`crate::csg::extrude_contributors`]), and warps inherit their inner leaf's
/// half-space band, so both report both branches at a true edge just like
/// analytic CSG. The special-case is gone.
fn both_branches_active(
    body: &BoundedCsg,
    p: [f32; 3],
    ba: BranchId,
    bb: BranchId,
    eps: f32,
) -> bool {
    let (_, contributors) = body.evaluate(p, eps, EvalMode::Logical);
    let slice = contributors.as_slice();
    slice.contains(&ba) && slice.contains(&bb)
}

/// Cell diagonal length (f32).
fn cell_diag(cell: &OctreeCell) -> f32 {
    let s = cell.aabb.size();
    (s[0] * s[0] + s[1] * s[1] + s[2] * s[2]).sqrt()
}

// ─── edge extraction ─────────────────────────────────────────────────────────

/// Surface-distance residual of an edge/vertex estimate: the magnitude of the
/// logical field. On a true edge/vertex every contributing branch is
/// simultaneously zero, so the logical (hard min/max) value is zero; off the
/// feature it is the signed distance to the nearest active surface. This is
/// exactly the acceptance quantity from the concept's phantom test and the
/// quantity the outer refinement drives to zero.
fn edge_residual(body: &BoundedCsg, p: [f32; 3]) -> f32 {
    surface_gradient(body, p).0.abs()
}

/// Solve the 3×3 system placing a point on the intersection line of two
/// tangent planes, with the free in-line DOF pinned to `anchor` along the
/// edge tangent `t = n_a × n_b`:
///
/// ```text
///   n_a · x = n_a · p_A
///   n_b · x = n_b · p_B
///   t   · x = t   · anchor
/// ```
///
/// Returns `(point, unit_tangent)` or `None` for parallel normals / a
/// singular system.
fn solve_edge_point(
    pa: &ProjectedPoint,
    pb: &ProjectedPoint,
    anchor: [f32; 3],
) -> Option<([f32; 3], Vector3<f32>)> {
    let na = Vector3::new(pa.normal[0], pa.normal[1], pa.normal[2]);
    let nb = Vector3::new(pb.normal[0], pb.normal[1], pb.normal[2]);

    let t = na.cross(&nb);
    let t_len = t.norm();
    if t_len < TINY_TANGENT {
        // Parallel / tangential normals → no well-defined edge (should have
        // been merged by G1 preprocessing).
        return None;
    }
    let t = t / t_len;

    let av = Vector3::new(anchor[0], anchor[1], anchor[2]);
    let pav = Vector3::new(pa.position[0], pa.position[1], pa.position[2]);
    let pbv = Vector3::new(pb.position[0], pb.position[1], pb.position[2]);

    let m = Matrix3::from_rows(&[na.transpose(), nb.transpose(), t.transpose()]);
    let rhs = Vector3::new(na.dot(&pav), nb.dot(&pbv), t.dot(&av));

    if m.determinant().abs() < TINY_DET {
        return None;
    }
    let x = m.try_inverse()? * rhs;
    Some(([x[0], x[1], x[2]], t))
}

/// Extract the edge point for one `Edge` cell (exactly 2 colliding branches).
///
/// Returns `None` when the cell is not an extractable edge: wrong class /
/// branch count, no clean per-branch start point, degenerate tangent
/// (parallel normals), a singular plane-intersection system, or a failed
/// phantom test.
pub fn extract_edge_point(
    body: &BoundedCsg,
    cell: &OctreeCell,
    halfspaces: &HalfspaceMap,
) -> Option<EdgePoint> {
    if cell.class != CellClass::Edge || cell.branches.len() != 2 {
        return None;
    }
    let ba = cell.branches[0];
    let bb = cell.branches[1];

    let c = cell.center();
    let diag = cell_diag(cell);

    // Initial per-branch tangent planes from clean cell-corner seeds.
    let mut pa = project_branch(body, cell, ba, halfspaces)?;
    let mut pb = project_branch(body, cell, bb, halfspaces)?;

    // Initial one-shot solve, anchored at the cell centre (DL-38). A `None` here
    // is the bleed-cell collapse (DL-79, sibling of the DL-78 vertex bug): the
    // cell straddles only ONE branch's surface and the other branch is merely an
    // eps-runner-up whose face sits just *outside* the cell, so its in-cell Newton
    // projection slid onto the present face — the two tangent normals coincide and
    // their cross product (the edge tangent) degenerates. This happens when a
    // curved union partner inflates the body AABB (coarser, grid-shifted octree),
    // leaving a clean box edge sampled by cells that only graze one of its faces.
    // Re-seed both branches from an outward sole-contributor probe (generic, no
    // analytic half-spaces) and re-solve; the residual/phantom/both-active gates
    // below then validate the rescued point exactly as for a normal cell.
    let (mut p_edge, mut t) = match solve_edge_point(&pa, &pb, c) {
        Some(s) => s,
        None => {
            let rescued = reproject_branches_outward(body, cell, diag);
            let pa_r = rescued.iter().find(|(b, _)| *b == ba).map(|(_, p)| *p)?;
            let pb_r = rescued.iter().find(|(b, _)| *b == bb).map(|(_, p)| *p)?;
            let s = solve_edge_point(&pa_r, &pb_r, c)?;
            pa = pa_r;
            pb = pb_r;
            s
        }
    };

    // Outer refinement for warped (curved) faces: re-anchor each branch's
    // tangent plane near the current estimate and re-solve, anchoring the
    // free in-tangent DOF at the current point (not the cell centre) so the
    // line moves along the curved edge toward it. Planar faces are detected
    // as already-converged on the first residual check and skip the loop.
    let delta = diag * 0.25;
    for _ in 0..REFINE_MAX_ITER {
        if edge_residual(body, p_edge) < REFINE_EPS {
            break;
        }
        // Re-project each branch from a seed nudged into its region near p.
        // The in-plane "away from fault" directions are ±(t × n_i), built
        // from the most recent branch normals.
        let n_a_cur = Vector3::new(pa.normal[0], pa.normal[1], pa.normal[2]);
        let n_b_cur = Vector3::new(pb.normal[0], pb.normal[1], pb.normal[2]);
        let dir_a = t.cross(&n_a_cur);
        let dir_b = t.cross(&n_b_cur);
        let Some(ra) = reproject_branch_near(body, ba, p_edge, &[dir_a], delta, halfspaces) else {
            break;
        };
        let Some(rb) = reproject_branch_near(body, bb, p_edge, &[dir_b], delta, halfspaces) else {
            break;
        };
        let Some((p_new, t_new)) = solve_edge_point(&ra, &rb, p_edge) else {
            break;
        };
        pa = ra;
        pb = rb;
        p_edge = p_new;
        t = t_new;
    }

    // Hard acceptance gate (DL-38): drop cells whose refined point still lies
    // off the warped surface rather than emit an inaccurate edge point.
    if edge_residual(body, p_edge) >= ACCEPT_EPS {
        return None;
    }

    // Phantom filter (cell-relative tolerance, DL-5).
    let eps_phantom = diag * PHANTOM_EPS_FACTOR;
    if !passes_phantom_filter(body, p_edge, eps_phantom) {
        return None;
    }

    // Both-branches-active gate (DL-63, universal since DL-65): reject a
    // phantom seam that has slid onto one branch's free surface, where the
    // other branch never reaches zero. A genuine intersection curve has both
    // branches tie at zero here, so both appear in the contributor set.
    if !both_branches_active(body, p_edge, ba, bb, diag * BOTH_ACTIVE_EPS_FACTOR) {
        return None;
    }

    let signature = if ba.as_u32() <= bb.as_u32() {
        (ba, bb)
    } else {
        (bb, ba)
    };

    Some(EdgePoint {
        position: [p_edge[0] as f64, p_edge[1] as f64, p_edge[2] as f64],
        tangent: [t[0] as f64, t[1] as f64, t[2] as f64],
        signature,
    })
}

// ─── vertex extraction ───────────────────────────────────────────────────────

/// The coordinate axis least aligned with `n` (smallest `|n·e_i|`), used to
/// build an in-tangent-plane direction that never degenerates: removing `n`'s
/// component from this axis yields a non-zero vector for any unit `n`.
fn least_aligned_axis(n: Vector3<f32>) -> Vector3<f32> {
    let ax = n[0].abs();
    let ay = n[1].abs();
    let az = n[2].abs();
    if ax <= ay && ax <= az {
        Vector3::new(1.0, 0.0, 0.0)
    } else if ay <= az {
        Vector3::new(0.0, 1.0, 0.0)
    } else {
        Vector3::new(0.0, 0.0, 1.0)
    }
}

/// Least-squares intersection point of a set of tangent planes
/// `n_i · x = n_i · p_i`, via the 3×3 normal equations
/// `(Σ n_i n_iᵀ) x = Σ n_i (n_i · p_i)`, factorised by SVD for a robust
/// rank-deficiency guard (see DL-6). Returns `None` if the system does not
/// pin a unique point.
fn solve_vertex_point(planes: &[(BranchId, ProjectedPoint)]) -> Option<[f32; 3]> {
    let mut ata = Matrix3::zeros();
    let mut atb = Vector3::zeros();
    for (_, proj) in planes {
        let n = Vector3::new(proj.normal[0], proj.normal[1], proj.normal[2]);
        let pv = Vector3::new(proj.position[0], proj.position[1], proj.position[2]);
        let d = n.dot(&pv);
        ata += n * n.transpose();
        atb += n * d;
    }
    let svd = ata.svd(true, true);
    if svd.singular_values[2] < TINY_DET {
        return None;
    }
    let x = svd.solve(&atb, TINY_DET).ok()?;
    Some([x[0], x[1], x[2]])
}

/// Extract the vertex for one `Vertex` cell (≥3 colliding branches).
///
/// Solves the over-determined system `n_i · x = n_i · p_i` for all branches
/// `i` in the least-squares sense via the 3×3 normal equations
/// `(Σ n_i n_iᵀ) x = Σ n_i (n_i · p_i)`, factorised by SVD for a robust
/// singularity guard (see DL-6). Returns `None` when fewer than 3 branches
/// project cleanly, the normal-equation matrix is rank-deficient, or the
/// phantom test fails.
pub fn extract_vertex(
    body: &BoundedCsg,
    cell: &OctreeCell,
    halfspaces: &HalfspaceMap,
) -> Option<Vertex> {
    if cell.class != CellClass::Vertex || cell.branches.len() < 3 {
        return None;
    }

    // Initial per-branch tangent planes from clean cell-corner seeds.
    let mut planes: Vec<(BranchId, ProjectedPoint)> = Vec::with_capacity(cell.branches.len());
    for &branch in &cell.branches {
        if let Some(proj) = project_branch(body, cell, branch, halfspaces) {
            planes.push((branch, proj));
        }
    }
    if planes.len() < 3 {
        return None;
    }

    let diag = cell_diag(cell);

    // Initial one-shot LSQ solve. A `None` here is the convex-corner collapse
    // (DL-78, the original Box ∪ curved-partner bug): the in-cell probes all sit
    // on the multi-branch fault, so a *side* branch's Newton projection slid onto
    // an adjacent face — two tangent planes coincide and the normal-equation
    // matrix is rank-deficient. Re-seed every branch from an outward
    // sole-contributor probe (generic, no analytic half-spaces) and re-solve.
    let mut p_vert = match solve_vertex_point(&planes) {
        Some(p) => p,
        None => {
            let rescued = reproject_branches_outward(body, cell, diag);
            if rescued.len() < 3 {
                return None;
            }
            let p = solve_vertex_point(&rescued)?;
            // Vertex analog of the DL-63 both-branches gate: a *rescued* solve
            // must have EVERY colliding branch actually pass through the solved
            // point. The phantom filter below only checks the union surface
            // `|f(p)|`, which a spurious corner on a box edge still satisfies —
            // e.g. where a far cylinder wall was merely an eps-runner-up in the
            // cell's branch set (box ∖ drill). Requiring all branches live at
            // `p` rejects those without touching the genuine convex corners.
            let (_, c) = body.evaluate(p, diag * BOTH_ACTIVE_EPS_FACTOR, EvalMode::Logical);
            let active = c.as_slice();
            if !cell.branches.iter().all(|b| active.contains(b)) {
                return None;
            }
            planes = rescued;
            p
        }
    };

    // Outer refinement for warped (curved) faces (see DL-38): re-anchor each
    // branch's tangent plane near the current vertex estimate and re-solve.
    // Planar faces are detected as already-converged on the first residual
    // check and skip the loop.
    let delta = diag * 0.25;
    for _ in 0..REFINE_MAX_ITER {
        if edge_residual(body, p_vert) < REFINE_EPS {
            break;
        }
        let mut next: Vec<(BranchId, ProjectedPoint)> = Vec::with_capacity(planes.len());
        for (branch, proj) in &planes {
            let n = Vector3::new(proj.normal[0], proj.normal[1], proj.normal[2]);
            // A vertex has no single edge tangent, so explore the branch's
            // whole tangent plane: build an orthonormal in-plane basis (u, v)
            // and probe ±u and ±v to find the seed that lands cleanly in this
            // branch's region near the vertex.
            let axis = least_aligned_axis(n);
            let u = (axis - n * n.dot(&axis)).normalize();
            let v = n.cross(&u);
            match reproject_branch_near(body, *branch, p_vert, &[u, v], delta, halfspaces) {
                Some(r) => next.push((*branch, r)),
                None => next.push((*branch, *proj)),
            }
        }
        let Some(p_new) = solve_vertex_point(&next) else {
            break;
        };
        planes = next;
        p_vert = p_new;
    }

    // Hard acceptance gate (DL-38): drop cells whose refined point still lies
    // off the warped surface rather than emit an inaccurate vertex.
    if edge_residual(body, p_vert) >= ACCEPT_EPS {
        return None;
    }

    // Phantom filter (cell-relative tolerance, DL-5).
    let eps_phantom = diag * PHANTOM_EPS_FACTOR;
    if !passes_phantom_filter(body, p_vert, eps_phantom) {
        return None;
    }

    Some(Vertex::new(
        [p_vert[0] as f64, p_vert[1] as f64, p_vert[2] as f64],
        cell.branches.clone(),
    ))
}

// ─── whole-octree convenience ─────────────────────────────────────────────────

/// Extract every edge point and vertex from an [`Octree`]. Walks all `Edge`
/// and `Vertex` leaf cells, runs Phase 2 + Phase 3 on each, and returns the
/// surviving `(edge points, vertices)`. Non-extractable cells (no clean start
/// point, degenerate geometry, failed phantom test) are silently skipped.
pub fn extract_features(body: &BoundedCsg, octree: &Octree) -> (Vec<EdgePoint>, Vec<Vertex>) {
    extract_features_with(body, octree, false)
}

/// Like [`extract_features`], but with an explicit `pure_octree` toggle.
///
/// When `pure_octree == true` the analytic DL-75 [`HalfspaceMap`] is forced
/// **empty**, disabling the analytic branch-projection fast-path even for
/// shelled bodies. Everything else is identical. This yields a purely numeric
/// octree extraction, needed as a clean reference for the differential oracle
/// (Phase 0b of the parametric-weaving rebuild): comparing analytic-vs-analytic
/// inside shelled regions would mask exactly the analytic bugs we want to catch.
pub fn extract_features_with(
    body: &BoundedCsg,
    octree: &Octree,
    pure_octree: bool,
) -> (Vec<EdgePoint>, Vec<Vertex>) {
    // Analytic branch-projection table — only for shelled bodies, whose twin
    // parallel surfaces defeat the generic body-SDF projection (DL-75). Empty
    // otherwise, so every non-shell weave keeps its exact previous numerics.
    // Forced empty in pure-octree mode (no analytic fast-path at all).
    let halfspaces = if !pure_octree && body_contains_shell(body) {
        crate::brep::weaving::branch_halfspace_map(body)
    } else {
        HalfspaceMap::new()
    };
    let edges = octree
        .edge_cells()
        .filter_map(|cell| extract_edge_point(body, cell, &halfspaces))
        .collect();
    let vertices = octree
        .vertex_cells()
        .filter_map(|cell| extract_vertex(body, cell, &halfspaces))
        .collect();
    (edges, vertices)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::csg::{Aabb, CsgNode};
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

    /// Is a unit vector axis-aligned (one component ≈ ±1, others ≈ 0)?
    fn is_axis_aligned(v: [f64; 3], tol: f64) -> bool {
        let mut near_one = 0;
        let mut near_zero = 0;
        for c in v {
            if (c.abs() - 1.0).abs() < tol {
                near_one += 1;
            } else if c.abs() < tol {
                near_zero += 1;
            }
        }
        near_one == 1 && near_zero == 2
    }

    #[test]
    fn box_edge_points_lie_on_surface_with_axis_tangents() {
        let body = box_body([1.0, 1.0, 1.0]);
        let octree = Octree::build(&body, 0.25);

        let mut count = 0;
        let mut signatures = HashSet::new();
        for cell in octree.edge_cells() {
            if let Some(ep) = extract_edge_point(&body, cell, &HalfspaceMap::new()) {
                count += 1;
                // On the surface.
                let p = [
                    ep.position[0] as f32,
                    ep.position[1] as f32,
                    ep.position[2] as f32,
                ];
                let (f, _) = surface_gradient(&body, p);
                assert!(
                    f.abs() < 1e-3,
                    "edge point off surface: f = {f}, p = {:?}",
                    ep.position
                );
                // Box edges run along an axis → axis-aligned tangent.
                assert!(
                    is_axis_aligned(ep.tangent, 1e-3),
                    "edge tangent not axis-aligned: {:?}",
                    ep.tangent
                );
                signatures.insert((ep.signature.0.as_u32(), ep.signature.1.as_u32()));
            }
        }
        assert!(count > 0, "no edge points extracted");
        // Resolution-robust: a cube has exactly 12 edge signatures.
        assert_eq!(
            signatures.len(),
            12,
            "expected 12 cube edges, got {signatures:?}"
        );
    }

    #[test]
    fn box_vertices_at_corners_with_three_branches() {
        let body = box_body([1.0, 1.0, 1.0]);
        let octree = Octree::build(&body, 0.25);

        // Collect one extracted vertex per distinct branch triple.
        let mut by_branches: std::collections::HashMap<Vec<u32>, [f64; 3]> =
            std::collections::HashMap::new();
        for cell in octree.vertex_cells() {
            if let Some(v) = extract_vertex(&body, cell, &HalfspaceMap::new()) {
                assert!(
                    v.branches.len() >= 3,
                    "vertex with <3 branches: {:?}",
                    v.branches
                );
                let key: Vec<u32> = v.branches.iter().map(|b| b.as_u32()).collect();
                by_branches.insert(key, v.position);
            }
        }

        // 8 cube corners.
        assert_eq!(by_branches.len(), 8, "expected 8 cube corners");

        // Each extracted vertex sits at a (±1, ±1, ±1) corner.
        let mut corners = HashSet::new();
        for pos in by_branches.values() {
            for c in *pos {
                assert!(
                    (c.abs() - 1.0).abs() < 1e-2,
                    "vertex not at corner: {pos:?}"
                );
            }
            let signs = (
                pos[0].signum() as i32,
                pos[1].signum() as i32,
                pos[2].signum() as i32,
            );
            corners.insert(signs);
        }
        assert_eq!(corners.len(), 8, "expected 8 distinct corner positions");
    }

    #[test]
    fn extract_features_convenience() {
        let body = box_body([1.0, 1.0, 1.0]);
        let octree = Octree::build(&body, 0.25);
        let (edges, vertices) = extract_features(&body, &octree);

        let edge_sigs: HashSet<_> = edges
            .iter()
            .map(|e| (e.signature.0.as_u32(), e.signature.1.as_u32()))
            .collect();
        let vert_sets: HashSet<Vec<u32>> = vertices
            .iter()
            .map(|v| v.branches.iter().map(|b| b.as_u32()).collect())
            .collect();
        assert_eq!(edge_sigs.len(), 12, "12 cube edges");
        assert_eq!(vert_sets.len(), 8, "8 cube corners");
    }

    #[test]
    fn phantom_filter_rejects_off_surface_keeps_on_surface() {
        let body = box_body([1.0, 1.0, 1.0]);
        let eps_phantom = 0.1;

        // On-surface point: the +X face centre (f ≈ 0) → kept.
        assert!(
            passes_phantom_filter(&body, [1.0, 0.0, 0.0], eps_phantom),
            "on-surface point should pass"
        );

        // Deep interior point (f ≈ -1, medial-axis-like) → rejected.
        assert!(
            !passes_phantom_filter(&body, [0.0, 0.0, 0.0], eps_phantom),
            "deep interior point should be rejected"
        );

        // Far exterior point (f ≫ 0, external trench) → rejected.
        assert!(
            !passes_phantom_filter(&body, [5.0, 5.0, 5.0], eps_phantom),
            "far exterior point should be rejected"
        );
    }

    #[test]
    fn non_edge_cell_yields_none() {
        let body = box_body([1.0, 1.0, 1.0]);
        let face_cell = OctreeCell {
            aabb: Aabb {
                min: [0.9, -0.1, -0.1],
                max: [1.1, 0.1, 0.1],
            },
            class: CellClass::Face,
            branches: vec![BranchId(1 << 4)],
        };
        assert!(extract_edge_point(&body, &face_cell, &HalfspaceMap::new()).is_none());
        assert!(extract_vertex(&body, &face_cell, &HalfspaceMap::new()).is_none());
    }

    #[test]
    fn difference_body_internal_collision_is_filtered() {
        // A box with a smaller box subtracted: the subtraction creates real
        // edges on the surface, but interior branch ties (e.g. where the
        // cutter's planes meet deep inside material) must be phantom-filtered.
        // We assert every surviving edge point lies on the actual surface.
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
        let (edges, vertices) = extract_features(&body, &octree);

        // Every surviving feature point must actually lie on the surface.
        for e in &edges {
            let p = [
                e.position[0] as f32,
                e.position[1] as f32,
                e.position[2] as f32,
            ];
            let (f, _) = surface_gradient(&body, p);
            assert!(
                f.abs() < 5e-2,
                "edge phantom slipped through: f = {f}, p = {:?}",
                e.position
            );
        }
        for v in &vertices {
            let p = [
                v.position[0] as f32,
                v.position[1] as f32,
                v.position[2] as f32,
            ];
            let (f, _) = surface_gradient(&body, p);
            assert!(
                f.abs() < 5e-2,
                "vertex phantom slipped through: f = {f}, p = {:?}",
                v.position
            );
        }
    }

    /// Branch id of the analytic primitive face nearest `p` on the union
    /// surface (for asserting which box face a seam lies on).
    fn union_box_sphere(box_half: [f32; 3], sph_center: [f32; 3], sph_r: f32) -> BoundedCsg {
        let boxn = CsgNode::Leaf {
            id: FeatureId::from_u32(1),
            primitive: Primitive::Box {
                center: [0.0, 0.0, 0.0],
                half_extents: box_half,
            },
        };
        let sph = CsgNode::Leaf {
            id: FeatureId::from_u32(2),
            primitive: Primitive::Sphere {
                center: sph_center,
                radius: sph_r,
            },
        };
        BoundedCsg::from_csg(&CsgNode::Union(Box::new(boxn), Box::new(sph)))
    }

    /// DL-63: the both-branches-active gate kills phantom seam points. A sphere
    /// poking through one box face must produce a seam that lies *exactly* on
    /// that face's plane — no points curling onto the sphere's free surface off
    /// the plane (the "edge curves inward toward the sphere" bug).
    #[test]
    fn union_seam_lies_on_the_box_face_not_the_free_sphere() {
        // Sphere centred 12 above the +Z face (z=10), r=8 → a clean circular
        // intersection on z=10 of radius √(64−4)=7.75, well inside the face.
        let body = union_box_sphere([10.0, 10.0, 10.0], [0.0, 0.0, 12.0], 8.0);
        let octree = Octree::build(&body, 0.5);
        let (edges, _) = extract_features(&body, &octree);

        // The sphere branch is the lone Sphere leaf; every edge it participates
        // in is a box-face∩sphere seam. Each such point must sit on z=10.
        let sphere_seam: Vec<_> = edges
            .iter()
            .filter(|e| e.signature.0.as_u32() == 8192 || e.signature.1.as_u32() == 8192)
            .collect();
        assert!(!sphere_seam.is_empty(), "no sphere seam extracted");
        for e in &sphere_seam {
            assert!(
                (e.position[2] - 10.0).abs() < 0.3,
                "seam point off the +Z face plane (phantom curl): {:?}",
                e.position
            );
        }
    }

    /// DL-64: a sphere poking straight through one face produces a *single
    /// continuous* circular seam, not dozens of fragments. The full circle
    /// weaves into exactly one closed-loop edge.
    #[test]
    fn union_seam_is_one_continuous_loop_not_fragments() {
        use crate::brep::weaving::weave;
        let body = union_box_sphere([10.0, 10.0, 10.0], [0.0, 0.0, 12.0], 8.0);
        let octree = Octree::build(&body, 0.5);
        let brep = weave(&body, &octree);

        let seam_edges: Vec<_> = brep
            .edges
            .iter()
            .filter(|e| e.signature.0.as_u32() == 8192 || e.signature.1.as_u32() == 8192)
            .collect();
        assert_eq!(
            seam_edges.len(),
            1,
            "sphere seam must be ONE continuous edge, got {} fragments",
            seam_edges.len()
        );
        // A full ring closes on itself: no bounding vertices.
        let e = seam_edges[0];
        assert!(
            e.start.is_none() && e.end.is_none(),
            "a closed circular seam carries no end vertices"
        );
    }

    // ─── D.3: domain warps (twist / bend) ──────────────────────────────────

    /// Box twisted around the local Y axis at `rate` rad/mm (DL-38 fixtures).
    fn twisted_box(half: [f32; 3], rate: f32) -> BoundedCsg {
        let leaf = CsgNode::Leaf {
            id: FeatureId::from_u32(1),
            primitive: Primitive::Box {
                center: [0.0, 0.0, 0.0],
                half_extents: half,
            },
        };
        BoundedCsg::from_csg(&CsgNode::Twist(Box::new(leaf), rate))
    }

    /// Box bent in the XY plane along X at `rate` rad/mm.
    fn bent_box(half: [f32; 3], rate: f32) -> BoundedCsg {
        let leaf = CsgNode::Leaf {
            id: FeatureId::from_u32(1),
            primitive: Primitive::Box {
                center: [0.0, 0.0, 0.0],
                half_extents: half,
            },
        };
        BoundedCsg::from_csg(&CsgNode::Bend(Box::new(leaf), rate))
    }

    /// Is a unit vector? (extracted edge tangents must be normalised).
    fn is_unit(v: [f64; 3], tol: f64) -> bool {
        let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        (len - 1.0).abs() < tol
    }

    /// Run extraction on `body` and assert the full B-Rep topology survives a
    /// smooth domain warp: every extracted edge/vertex point lies on the warped
    /// surface (`|f| < ACCEPT_EPS_TEST`), edge tangents are unit, and the
    /// extraction finds exactly 12 distinct edge signatures + 8 vertex
    /// branch-sets (a warped cube is still combinatorially a cube).
    ///
    /// `ACCEPT_EPS_TEST` is `5e-3`: a small slack over the `ACCEPT_EPS = 1e-3`
    /// extractor gate to absorb the f32 re-evaluation round-off in the assert
    /// itself. Justified in DL-38.
    fn assert_warped_cube_topology(body: &BoundedCsg, min_cell: f32) {
        const ACCEPT_EPS_TEST: f32 = 5e-3;
        let octree = Octree::build(body, min_cell);
        let (edges, vertices) = extract_features(body, &octree);

        let mut edge_sigs = HashSet::new();
        for e in &edges {
            let p = [
                e.position[0] as f32,
                e.position[1] as f32,
                e.position[2] as f32,
            ];
            let (f, _) = surface_gradient(body, p);
            assert!(
                f.abs() < ACCEPT_EPS_TEST,
                "warped edge point off surface: f = {f}, p = {:?}",
                e.position
            );
            assert!(
                is_unit(e.tangent, 1e-4),
                "edge tangent not unit: {:?}",
                e.tangent
            );
            edge_sigs.insert((e.signature.0.as_u32(), e.signature.1.as_u32()));
        }

        let mut vert_sets = HashSet::new();
        for v in &vertices {
            let p = [
                v.position[0] as f32,
                v.position[1] as f32,
                v.position[2] as f32,
            ];
            let (f, _) = surface_gradient(body, p);
            assert!(
                f.abs() < ACCEPT_EPS_TEST,
                "warped vertex off surface: f = {f}, p = {:?}",
                v.position
            );
            assert!(
                v.branches.len() >= 3,
                "vertex with <3 branches: {:?}",
                v.branches
            );
            let key: Vec<u32> = v.branches.iter().map(|b| b.as_u32()).collect();
            vert_sets.insert(key);
        }

        assert_eq!(
            edge_sigs.len(),
            12,
            "expected 12 warped-cube edges, got {edge_sigs:?}"
        );
        assert_eq!(
            vert_sets.len(),
            8,
            "expected 8 warped-cube corners, got {vert_sets:?}"
        );
    }

    #[test]
    fn twisted_cube_extracts_correct_brep_edges() {
        // A visibly-warped 2×2×2 cube: 0.2 rad/mm ≈ 11.5°/mm twist.
        assert_warped_cube_topology(&twisted_box([1.0, 1.0, 1.0], 0.2), 0.25);
    }

    #[test]
    fn strongly_twisted_cube_extracts_correct_brep_edges() {
        // Stronger twist (0.4 rad/mm ≈ 23°/mm): the one-shot solve was off by
        // up to ~0.067 here (see DL-38) — refinement + gate must recover it.
        assert_warped_cube_topology(&twisted_box([1.0, 1.0, 1.0], 0.4), 0.25);
    }

    #[test]
    fn bent_cube_extracts_correct_brep_edges() {
        // Bend variant of the same acceptance criterion.
        assert_warped_cube_topology(&bent_box([1.0, 1.0, 1.0], 0.3), 0.25);
    }

    /// DL-66: a sphere poking through a box *corner* crosses the three box edges
    /// meeting there at near-tangential angles (the sphere normal runs nearly
    /// parallel to each box edge at the crossing). The per-cell vertex LSQ then
    /// scatters and the centroid drifts off the true corner, so before DL-66 the
    /// converging seam arcs failed to snap and three faces fell back to unordered
    /// loops. The analytic triple-vertex re-solve must place each crossing on the
    /// exact corner so every face weaves into a connected ring with no dangling
    /// edge.
    #[test]
    fn sphere_over_box_corner_weaves_valid_faces() {
        use crate::brep::weaving::weave;
        // Sphere centred on the +X+Y+Z corner (10,10,10), r=8. On the +X face
        // (x=10) the seam is the circle (y−10)²+(z−10)²=64; it meets the +X/+Y
        // box edge (x=y=10) at z=10−8=2 → the triple vertex {+X,+Y,sphere} sits
        // exactly at (10,10,2), and by symmetry the other two at (10,2,10) and
        // (2,10,10).
        let body = union_box_sphere([10.0, 10.0, 10.0], [10.0, 10.0, 10.0], 8.0);
        let octree = Octree::build(&body, 0.5);
        let brep = weave(&body, &octree);

        // The three near-tangential triple junctions must land on the exact
        // analytic corners (a centroid drift would be ≫ 0.1 here).
        for want in [[10.0, 10.0, 2.0], [10.0, 2.0, 10.0], [2.0, 10.0, 10.0]] {
            let hit = brep.vertices.iter().any(|v| {
                let d = (0..3)
                    .map(|i| (v.position[i] - want[i]).powi(2))
                    .sum::<f64>()
                    .sqrt();
                d < 0.1
            });
            assert!(hit, "no triple vertex near tangential corner {want:?}");
        }

        // No face may fall back to the unordered loop: every outer/inner loop is
        // a connected ring (the watertight-topology guarantee). A drifted corner
        // would leave a dangling endpoint and break the ring here.
        for (fi, f) in brep.faces.iter().enumerate() {
            assert!(
                f.outer.is_connected_ring(&brep.edges),
                "face {fi} (branch {}) outer loop is not a connected ring",
                f.branch_id.as_u32()
            );
            for (li, lp) in f.inner.iter().enumerate() {
                assert!(
                    lp.is_connected_ring(&brep.edges),
                    "face {fi} inner loop {li} is not a connected ring"
                );
            }
        }

        // No vertex-bounded edge may dangle: each edge either closes on itself
        // (a full trim circle) or carries both endpoints.
        for e in &brep.edges {
            let both = e.start.is_some() && e.end.is_some();
            let loop_edge = e.start.is_none() && e.end.is_none();
            assert!(
                both || loop_edge,
                "dangling edge sig=({},{}) start={:?} end={:?}",
                e.signature.0.as_u32(),
                e.signature.1.as_u32(),
                e.start,
                e.end
            );
        }
    }
}
