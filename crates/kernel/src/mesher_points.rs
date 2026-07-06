//! Debug ▸ Mesher — iterative point optimisation on the narrow-band octree
//! (ticket jq2t).
//!
//! The octree scaffold (`api::export::octree_leaves`) yields exactly one
//! point per leaf cell (its centre). This module distributes a FILTERED
//! subset of those points, entirely kernel-side (a JS round-trip per SDF eval
//! would be hopeless at 200k pts). The atomic ops (each applied only when its
//! checkbox is set):
//!
//!  - **Fit on Surface** — damped Newton step toward the SDF zero set:
//!    `p ← p − stepMultiplier · d · ∇f/|∇f|`.
//!  - **Clamp to Cell** — clamp `p` to its origin leaf's bounds.
//!  - **Repulsion inside Surface** — tangential neighbour-repulsion step
//!    (uniform-grid hash within `interactionRadius`, force projected into the
//!    surface tangent plane). The pairwise force follows an inverse-square law
//!    (`∝ 1/d²`, like equally-charged monopoles): closer points repel far
//!    harder, so clusters break up and the interaction stays local — an even,
//!    clump-free spread rather than a global drift into the corners.
//!
//! ONE interleaved relaxation loop (ticket yanv): per iteration
//! **Repulsion → Fit → Clamp**, for `iterations`, with the repulsion
//! strength ANNEALED from full down to ~0 across the iterations — early steps
//! spread aggressively, the fit dominates at the end. A final **Fit → Clamp**
//! convergence (to `targetEpsilon` / `iterations`) then snaps each point
//! ONTO the surface within its cell. With repulsion off, that final pass IS
//! the pure Fit-on-Surface.
//!
//! Cross-cutting:
//!  - `constrainToCell` keeps one point per cell one point per cell
//!    (topological protection) — the Clamp op in the loop and the snap.
//!  - Processing filters (AND when both active): `nearSurfaceOnly`
//!    (`|sdf| ≤ nearSurfaceDist`) and `signChangeNeighboursOnly` — only cells
//!    where the field sign flips towards a direct neighbour. The neighbour
//!    sign is probed at `centre ± axis·size` (where an equal-size neighbour's
//!    centre sits) — resolution-independent and adjacency-structure-free.
//!    Points that fail the filters stay rigid at their cell centre.

use serde::Deserialize;

/// One octree leaf: centre sample + cell size (bounds = centre ± size/2).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Leaf {
    pub center: [f32; 3],
    pub sdf: f32,
    pub normal: [f32; 3],
    pub size: f32,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MesherPointOptions {
    pub max_depth: u32,
    pub band_guard: f32,
    // Processing filters (AND).
    pub near_surface_only: bool,
    pub near_surface_dist: f32,
    pub sign_change_neighbours_only: bool,
    // Topological protection.
    pub constrain_to_cell: bool,
    /// Total iterations of the single relaxation loop (also the max budget of
    /// the final fit-to-surface snap). One loop now — not two separate counts.
    pub iterations: u32,
    // Fit on Surface.
    pub fit_on_surface: bool,
    /// Drop points that did NOT actually reach the surface after the fit
    /// (typically clamped at their cell bounds): they leave the point set
    /// entirely — not drawn, not repelled, their cells emit no mesh vertex.
    pub only_on_surface: bool,
    pub fit_target_epsilon: f32,
    pub fit_step_multiplier: f32,
    // Repulsion inside surface (tangential relaxation).
    pub repulsion: bool,
    pub interaction_radius: f32,
    pub repulsion_strength: f32,
    /// Annealing: fraction of repulsion strength lost per iteration
    /// (`0` = constant, `1` = gone after one linear step). Same meaning for
    /// both damping curves.
    pub repulsion_damping: f32,
    /// Annealing curve of the repulsion strength over the iterations.
    pub damping_curve: DampingCurve,
    /// Hybrid-PASM P2: use the paper's isotropic Gaussian pair kernel instead
    /// of the 1/d² monopole — force `(xi−xj)/2σ² · exp(−d²/4σ²)` with
    /// `σ = interactionRadius/5` (the paper truncates the kernel at 5σ, so
    /// the existing grid cutoff keeps its meaning). Smooth equilibrium at
    /// uniform spacing; pairs beyond ~3σ contribute almost nothing.
    pub pasm_kernel: bool,
    /// Hybrid-PASM P3: triangulate PLANAR faces with a constrained Delaunay
    /// triangulation in the face plane — the skeleton loops as constraints,
    /// the free particles as interior vertices. Watertight across seams by
    /// construction (adjacent faces share the same chain slots). Requires
    /// `brep_skeleton`; replaces the octree-connectivity debug mesh.
    pub cdt_faces: bool,
    /// Hybrid-PASM P1 (docs/HYBRID-PASM-MESHING-PLAN.md, ticket nwmi): sample
    /// the body's woven B-Rep edges into FIXED skeleton particles (corners
    /// pinned exactly, edge chains at the target spacing). They repel the
    /// free surface particles but never move — sharp features become
    /// boundary conditions instead of a detection problem.
    pub brep_skeleton: bool,
    // Feature snapping (edges/corners) — ticket-less follow-up of l45r:
    // isotropic repulsion NEVER lands points on 0D/1D features (they have
    // measure zero and the tangential projection surfs around the crease),
    // so features are snapped actively: probe the normal field around each
    // point, detect distinct sheets, pull the point onto the sheets'
    // intersection (edge line / corner point) and LOCK it there — edge
    // points then relax 1D along their edge, corners freeze.
    pub feature_snap: bool,
    /// Two probed normals differing by MORE than this angle (degrees) count
    /// as distinct sheets (edge/corner detection threshold).
    pub feature_angle_deg: f32,
    // Meshing over the optimised points (octree-edge connectivity).
    pub mesh_points: bool,
    /// Enforce the 2:1 octree balance before meshing (bounded depth
    /// contrast between face neighbours → more even triangle sizes).
    pub balance_octree: bool,
}

/// How the repulsion strength decays over the relaxation iterations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DampingCurve {
    /// `strength · max(0, 1 − damping · t)` — vanishes after `1/damping` steps.
    Linear,
    /// `strength · (1 − damping)^t` — geometric, asymptotic to 0.
    Exponential,
}

impl Default for MesherPointOptions {
    fn default() -> Self {
        Self {
            max_depth: 6,
            band_guard: 1.0,
            near_surface_only: false,
            near_surface_dist: 1.0,
            sign_change_neighbours_only: false,
            constrain_to_cell: true,
            iterations: 12,
            fit_on_surface: false,
            only_on_surface: false,
            fit_target_epsilon: 0.001,
            fit_step_multiplier: 0.8,
            repulsion: false,
            interaction_radius: 2.0,
            repulsion_strength: 0.5,
            repulsion_damping: 0.2,
            damping_curve: DampingCurve::Linear,
            pasm_kernel: false,
            cdt_faces: false,
            brep_skeleton: false,
            feature_snap: false,
            feature_angle_deg: 30.0,
            mesh_points: false,
            balance_octree: true,
        }
    }
}

/// Motion constraint of a point after feature snapping.
#[derive(Debug, Clone, Copy, PartialEq)]
enum PointLock {
    /// Repulsion moves the point in its surface tangent plane (2D).
    Free,
    /// On an edge: moves only along the edge tangent `n1×n2` (1D), re-snapped
    /// onto the two sheets' intersection after every move.
    Edge,
    /// On a corner (≥3 sheets): fully pinned (0D). Still repels neighbours.
    Corner,
}

/// One detected sheet cluster around a point: averaged unit normal plus a
/// surface anchor (a probe point projected onto that sheet) — together they
/// define the sheet's tangent plane for the intersection solve.
struct SheetCluster {
    normal: [f32; 3],
    anchor: [f32; 3],
    count: f32,
}

fn v_add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn v_sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn v_scale(a: [f32; 3], s: f32) -> [f32; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn v_dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn v_cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn v_len(a: [f32; 3]) -> f32 {
    v_dot(a, a).sqrt()
}
fn v_norm(a: [f32; 3]) -> [f32; 3] {
    let l = v_len(a);
    if l > 1e-12 { v_scale(a, 1.0 / l) } else { a }
}

/// Probe the normal field in the tangent plane around `p` (8 directions at
/// `probe_r`, each probe Newton-projected back onto the surface) and greedily
/// cluster the normals by `cos_thresh`. One cluster = smooth sheet; two =
/// edge; three or more = corner.
fn detect_sheets(
    p: [f32; 3],
    probe_r: f32,
    cos_thresh: f32,
    sdf: &dyn Fn([f32; 3]) -> f32,
    grad: &dyn Fn([f32; 3]) -> [f32; 3],
) -> Vec<SheetCluster> {
    let n0 = grad(p);
    // Tangent basis.
    let pick = if n0[0].abs() < 0.9 { [1.0, 0.0, 0.0] } else { [0.0, 1.0, 0.0] };
    let u = v_norm(v_cross(n0, pick));
    let v = v_norm(v_cross(n0, u));
    let s = std::f32::consts::FRAC_1_SQRT_2;
    let dirs = [
        u,
        v_scale(u, -1.0),
        v,
        v_scale(v, -1.0),
        v_norm(v_add(v_scale(u, s), v_scale(v, s))),
        v_norm(v_add(v_scale(u, -s), v_scale(v, s))),
        v_norm(v_add(v_scale(u, s), v_scale(v, -s))),
        v_norm(v_add(v_scale(u, -s), v_scale(v, -s))),
    ];
    // Cluster ONLY the projected probes: they land on the smooth sheets. The
    // point's own normal `n0` is deliberately excluded — exactly ON a crease
    // the (FD) gradient is a blend of the adjacent sheets (e.g. the diagonal
    // at a box corner) and would seed a phantom sheet that corrupts the
    // intersection solve.
    let mut clusters: Vec<SheetCluster> = Vec::new();
    for d in dirs {
        let mut q = v_add(p, v_scale(d, probe_r));
        // Newton-project the probe back onto the surface.
        for _ in 0..3 {
            let dq = sdf(q);
            let g = grad(q);
            q = v_sub(q, v_scale(g, dq));
        }
        // A probe that wandered far off (open space / other sheet across a
        // thin wall) is not a local sheet sample.
        if v_len(v_sub(q, p)) > 2.5 * probe_r || sdf(q).abs() > 0.25 * probe_r {
            continue;
        }
        let n = grad(q);
        match clusters
            .iter_mut()
            .find(|c| v_dot(v_norm(c.normal), n) > cos_thresh)
        {
            Some(c) => {
                let w = c.count;
                c.normal = v_norm(v_add(v_scale(c.normal, w), n));
                c.anchor = v_scale(v_add(v_scale(c.anchor, w), q), 1.0 / (w + 1.0));
                c.count = w + 1.0;
            }
            None => clusters.push(SheetCluster { normal: n, anchor: q, count: 1.0 }),
        }
    }
    // Strongest sheets first, and drop single-probe clusters: a probe landing
    // ON a crease carries a blended (FD) normal that matches no real sheet —
    // as a phantom cluster it would corrupt the intersection solve (a box
    // corner then "snaps" onto a blend plane instead of the third face). A
    // real sheet catches ≥2 of the 8 probes at every edge/corner geometry.
    clusters.sort_by(|a, b| b.count.total_cmp(&a.count));
    clusters.retain(|c| c.count >= 2.0);
    clusters
}

/// Pull `p` onto the intersection of the clusters' tangent planes — the
/// minimal-norm correction `x = p + Aᵀ(AAᵀ)⁻¹(b − Ap)` over 2 (edge line) or
/// 3 (corner point) planes. Returns `None` when the system is ill-conditioned
/// (near-parallel sheets — no real feature) or the correction exceeds
/// `max_step` (spurious detection). Curvature error is cleaned by the caller's
/// following fit step.
fn snap_to_sheet_intersection(
    p: [f32; 3],
    sheets: &[SheetCluster],
    max_step: f32,
) -> Option<[f32; 3]> {
    let k = sheets.len().min(3);
    let n: Vec<[f32; 3]> = sheets[..k].iter().map(|c| v_norm(c.normal)).collect();
    let r: Vec<f32> = sheets[..k]
        .iter()
        .zip(&n)
        .map(|(c, ni)| v_dot(*ni, v_sub(c.anchor, p)))
        .collect();
    let correction = if k == 2 {
        // 2×2 Gram solve; det = |n1×n2|² (the squared edge-tangent length).
        let (a, b, c) = (v_dot(n[0], n[0]), v_dot(n[0], n[1]), v_dot(n[1], n[1]));
        let det = a * c - b * b;
        if det < 1e-4 {
            return None;
        }
        let l0 = (c * r[0] - b * r[1]) / det;
        let l1 = (a * r[1] - b * r[0]) / det;
        v_add(v_scale(n[0], l0), v_scale(n[1], l1))
    } else {
        // 3×3 Gram solve via cofactors.
        let g = [
            [v_dot(n[0], n[0]), v_dot(n[0], n[1]), v_dot(n[0], n[2])],
            [v_dot(n[1], n[0]), v_dot(n[1], n[1]), v_dot(n[1], n[2])],
            [v_dot(n[2], n[0]), v_dot(n[2], n[1]), v_dot(n[2], n[2])],
        ];
        let det = g[0][0] * (g[1][1] * g[2][2] - g[1][2] * g[2][1])
            - g[0][1] * (g[1][0] * g[2][2] - g[1][2] * g[2][0])
            + g[0][2] * (g[1][0] * g[2][1] - g[1][1] * g[2][0]);
        if det.abs() < 1e-6 {
            return None;
        }
        let inv = |i: usize, j: usize| -> f32 {
            // Cofactor expansion of the symmetric Gram inverse.
            let (i1, i2) = ((i + 1) % 3, (i + 2) % 3);
            let (j1, j2) = ((j + 1) % 3, (j + 2) % 3);
            (g[i1][j1] * g[i2][j2] - g[i1][j2] * g[i2][j1]) / det
        };
        let mut l = [0.0f32; 3];
        for (i, li) in l.iter_mut().enumerate() {
            *li = (0..3).map(|j| inv(j, i) * r[j]).sum();
        }
        v_add(
            v_add(v_scale(n[0], l[0]), v_scale(n[1], l[1])),
            v_scale(n[2], l[2]),
        )
    };
    if v_len(correction) > max_step {
        return None;
    }
    Some(v_add(p, correction))
}

/// Resample a polyline by arc length at spacing `h`: both endpoints exact,
/// interior points at uniform arc-length steps (the last interval absorbs the
/// remainder, so spacing stays within [h/2, 1.5h] instead of emitting a
/// sliver point next to the end). Degenerate/short chains yield the endpoints.
pub(crate) fn resample_polyline(pts: &[[f32; 3]], h: f32) -> Vec<[f32; 3]> {
    if pts.len() < 2 {
        return pts.to_vec();
    }
    let seg = |a: [f32; 3], b: [f32; 3]| {
        ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2) + (b[2] - a[2]).powi(2)).sqrt()
    };
    let total: f32 = pts.windows(2).map(|w| seg(w[0], w[1])).sum();
    let h = h.max(1e-6);
    let steps = (total / h).round().max(1.0) as usize;
    let step = total / steps as f32;
    let mut out = Vec::with_capacity(steps + 1);
    out.push(pts[0]);
    let mut acc = 0.0f32;
    let mut next = step;
    for w in pts.windows(2) {
        let l = seg(w[0], w[1]);
        if l <= 1e-12 {
            continue;
        }
        while next <= acc + l && out.len() < steps {
            let t = (next - acc) / l;
            out.push([
                w[0][0] + t * (w[1][0] - w[0][0]),
                w[0][1] + t * (w[1][1] - w[0][1]),
                w[0][2] + t * (w[1][2] - w[0][2]),
            ]);
            next += step;
        }
        acc += l;
    }
    out.push(*pts.last().expect("len >= 2"));
    out
}

fn clamp_to_cell(p: &mut [f32; 3], leaf: &Leaf) {
    let h = leaf.size * 0.5;
    for a in 0..3 {
        p[a] = p[a].clamp(leaf.center[a] - h, leaf.center[a] + h);
    }
}

/// One damped-Newton "Fit on Surface" step: move `p` toward the SDF zero set
/// by `stepMultiplier · d · ∇f`. The atomic Fit op used throughout.
fn fit_step_once(
    p: &mut [f32; 3],
    sdf: &dyn Fn([f32; 3]) -> f32,
    grad: &dyn Fn([f32; 3]) -> [f32; 3],
    step_multiplier: f32,
) {
    let d = sdf(*p);
    let n = grad(*p);
    let step = step_multiplier * d;
    p[0] -= step * n[0];
    p[1] -= step * n[1];
    p[2] -= step * n[2];
}

/// One annealed repulsion relaxation (ticket yanv/l45r): per iteration a grid
/// hash over the current positions, 1/d² neighbour forces with the
/// half-nearest-neighbour stability clamp, then Fit → Clamp. `feature`
/// constrains motion after feature snapping: `Free` points move in their
/// surface tangent plane (2D), `Edge` points only along their edge tangent
/// (1D, re-detected each step and re-snapped onto the sheet intersection),
/// `Corner` points are pinned — but all of them still repel their neighbours.
/// `None` = every point Free (the plain relaxation).
fn relaxation_pass(
    points: &mut Vec<[f32; 3]>,
    leaves: &[Leaf],
    active: &[usize],
    opts: &MesherPointOptions,
    sdf: &dyn Fn([f32; 3]) -> f32,
    grad: &dyn Fn([f32; 3]) -> [f32; 3],
    feature: Option<(&[PointLock], f32)>,
    anchors: &[[f32; 3]],
) {
    let step = opts.fit_step_multiplier.clamp(0.1, 1.0);
    let r = opts.interaction_radius;
    let inv_cell = 1.0 / r;
    let key = |p: &[f32; 3]| -> (i32, i32, i32) {
        (
            (p[0] * inv_cell).floor() as i32,
            (p[1] * inv_cell).floor() as i32,
            (p[2] * inv_cell).floor() as i32,
        )
    };
    let iters = opts.iterations.max(1);
    let damping = opts.repulsion_damping.clamp(0.0, 1.0);
    for t in 0..iters {
        // Anneal: repulsion strongest first, decaying by the chosen curve.
        let tf = t as f32;
        let anneal = match opts.damping_curve {
            DampingCurve::Linear => (1.0 - damping * tf).max(0.0),
            DampingCurve::Exponential => (1.0 - damping).powi(t as i32),
        };
        let strength = opts.repulsion_strength * anneal;
        // Grid hash over the CURRENT positions of the active points, plus the
        // FIXED anchors (B-Rep skeleton) as virtual indices ≥ points.len() —
        // they repel every free particle but are never themselves moved.
        let n_pts = points.len();
        let at = |j: usize| -> [f32; 3] {
            if j < n_pts { points[j] } else { anchors[j - n_pts] }
        };
        let mut grid: std::collections::HashMap<(i32, i32, i32), Vec<usize>> =
            std::collections::HashMap::new();
        for &i in active {
            grid.entry(key(&points[i])).or_default().push(i);
        }
        for (a, p) in anchors.iter().enumerate() {
            grid.entry(key(p)).or_default().push(n_pts + a);
        }
        let mut moved = points.clone();
        let mut max_disp = 0.0f32;
        for &i in active {
            let lock = feature.map_or(PointLock::Free, |(l, _)| l[i]);
            if lock == PointLock::Corner {
                continue; // pinned (0D) — stays in the grid, repels others
            }
            let p = points[i];
            let (kx, ky, kz) = key(&p);
            let mut f = [0.0f32; 3];
            // Distance to the closest neighbour — the stability scale for
            // the explicit step below.
            let mut nearest = f32::MAX;
            for dz in -1..=1 {
                for dy in -1..=1 {
                    for dx in -1..=1 {
                        let Some(bucket) = grid.get(&(kx + dx, ky + dy, kz + dz)) else {
                            continue;
                        };
                        for &j in bucket {
                            if j == i {
                                continue;
                            }
                            let q = at(j);
                            let dvec = [p[0] - q[0], p[1] - q[1], p[2] - q[2]];
                            let dist = (dvec[0] * dvec[0]
                                + dvec[1] * dvec[1]
                                + dvec[2] * dvec[2])
                                .sqrt();
                            if dist >= r || dist < 1e-9 {
                                continue;
                            }
                            nearest = nearest.min(dist);
                            let s = if opts.pasm_kernel {
                                // PASM isotropic Gaussian kernel (paper Eq. 6,
                                // M = I): F = (xi−xj)/2σ²·exp(−d²/4σ²) with
                                // σ = r/5 so the grid cutoff = the paper's 5σ
                                // truncation. Smooth, principled equilibrium —
                                // no cap needed (the force VANISHES towards
                                // coincidence instead of exploding; cell-centre
                                // initialisation never starts coincident).
                                let sigma = r / 5.0;
                                (-(dist * dist) / (4.0 * sigma * sigma)).exp()
                                    / (2.0 * sigma * sigma)
                            } else {
                                // Inverse-square repulsion (`∝ 1/d²`, like
                                // equally-charged monopoles): magnitude
                                // normalised to 1 at the cutoff radius `r`
                                // (scale-relative) and capped so near-coincident
                                // points can't explode. Closer points repel far
                                // harder → clusters break up, the far field
                                // stays negligible → even, local spread.
                                ((r / dist) * (r / dist)).min(100.0) / dist
                            };
                            f[0] += dvec[0] * s;
                            f[1] += dvec[1] * s;
                            f[2] += dvec[2] * s;
                        }
                    }
                }
            }
            let leaf = &leaves[i];
            let mut np = p;
            let mut resnapped = false;
            // Repulsion: the neighbour force projected into the point's
            // allowed motion space, scaled by the annealed strength.
            if f != [0.0, 0.0, 0.0] {
                let ft = match lock {
                    PointLock::Free => {
                        // Tangential component (stays on the surface plane).
                        let n = grad(p);
                        let fn_dot = v_dot(f, n);
                        v_sub(f, v_scale(n, fn_dot))
                    }
                    PointLock::Edge => {
                        // 1D: along the freshly detected edge tangent n1×n2 —
                        // re-detecting each step follows CURVED edges instead
                        // of sliding off a stale chord direction.
                        let (_, cos_t) = feature.expect("edge lock implies feature");
                        let sheets = detect_sheets(p, 0.5 * leaf.size, cos_t, sdf, grad);
                        if sheets.len() >= 2 {
                            let tdir = v_norm(v_cross(sheets[0].normal, sheets[1].normal));
                            v_scale(tdir, v_dot(f, tdir))
                        } else {
                            // Feature vanished under the point (drifted off a
                            // rounded region) — fall back to the tangent plane.
                            let n = grad(p);
                            v_sub(f, v_scale(n, v_dot(f, n)))
                        }
                    }
                    PointLock::Corner => unreachable!("pinned above"),
                };
                let mut step_vec = v_scale(ft, strength);
                // Stability clamp (ticket l45r): never move a point more
                // than HALF the distance to its nearest neighbour. The 1/d²
                // force is stiff — a plain explicit-Euler step at high
                // strength leaps a point clean past its neighbours, off the
                // surface, where only-on-surface then drops it; the cluster
                // CORE (whose forces cancel by symmetry, net ≈ 0) never
                // moves, so the pile stays put ("clump in the corner"). The
                // half-neighbour cap guarantees a point can never overshoot
                // a neighbour, making the relaxation stable for ANY strength
                // — clumps expand monotonically toward even spacing.
                let step_len = v_len(step_vec);
                let max_step = 0.5 * nearest;
                if step_len > max_step && step_len > 1e-12 {
                    step_vec = v_scale(step_vec, max_step / step_len);
                }
                np = v_add(p, step_vec);
                // Edge points: re-snap onto the (moved-to) sheet intersection
                // so they STAY on the crease while sliding along it.
                if lock == PointLock::Edge {
                    let (_, cos_t) = feature.expect("edge lock implies feature");
                    let sheets = detect_sheets(np, 0.5 * leaf.size, cos_t, sdf, grad);
                    if sheets.len() >= 2 {
                        if let Some(x) = snap_to_sheet_intersection(np, &sheets, leaf.size) {
                            np = x;
                            resnapped = true;
                        }
                    }
                }
            }
            // Fit (single Newton step): nudge back onto the surface after
            // the tangential move. Skipped for a just-re-snapped edge point —
            // the fit would pull it off the crease onto the nearest sheet.
            if opts.fit_on_surface && !resnapped {
                fit_step_once(&mut np, sdf, grad, step);
            }
            if opts.constrain_to_cell {
                clamp_to_cell(&mut np, leaf);
            }
            let dx = np[0] - points[i][0];
            let dy = np[1] - points[i][1];
            let dz = np[2] - points[i][2];
            max_disp = max_disp.max(dx * dx + dy * dy + dz * dz);
            moved[i] = np;
        }
        *points = moved;
        // Converged (PASM stopping condition): when the largest particle
        // displacement drops below a thousandth of the interaction radius the
        // system is in equilibrium — further iterations only burn time.
        if max_disp.sqrt() < 1e-3 * r {
            break;
        }
    }
}

/// Run the jq2t point pipeline over the leaves. Returns the final position of
/// every leaf's point (unprocessed points stay at the cell centre) plus which
/// ones were processed. `sdf`/`grad` evaluate the scene field.
pub(crate) fn optimize_leaf_points(
    leaves: &[Leaf],
    opts: &MesherPointOptions,
    sdf: &dyn Fn([f32; 3]) -> f32,
    grad: &dyn Fn([f32; 3]) -> [f32; 3],
    skeleton: &[[f32; 3]],
) -> (Vec<[f32; 3]>, Vec<bool>, Vec<bool>) {
    let mut points: Vec<[f32; 3]> = leaves.iter().map(|l| l.center).collect();
    // "Only on surface": points that fail to reach the zero set drop OUT of
    // the set entirely (jq2t follow-up filter). All alive until proven not.
    let mut alive: Vec<bool> = vec![true; leaves.len()];

    // ── Processing filter (AND of the active ones) ──
    let mut process: Vec<bool> = Vec::with_capacity(leaves.len());
    for leaf in leaves {
        let mut ok = true;
        if opts.near_surface_only && leaf.sdf.abs() > opts.near_surface_dist {
            ok = false;
        }
        if ok && opts.sign_change_neighbours_only {
            // Probe where an equal-size neighbour's centre would sit.
            let my_sign = leaf.sdf >= 0.0;
            let mut flips = false;
            'probe: for axis in 0..3 {
                for dir in [-1.0f32, 1.0] {
                    let mut q = leaf.center;
                    q[axis] += dir * leaf.size;
                    if (sdf(q) >= 0.0) != my_sign {
                        flips = true;
                        break 'probe;
                    }
                }
            }
            ok = flips;
        }
        process.push(ok);
    }

    let step = opts.fit_step_multiplier.clamp(0.1, 1.0);
    let eps = opts.fit_target_epsilon.max(0.0);

    // ── Interleaved relaxation (ticket yanv) ──
    // ONE loop, per iteration: Repulsion → Fit → Clamp. The repulsion strength
    // is ANNEALED from full down to (nearly) zero across the iterations, so
    // early steps spread the points aggressively while later steps let the fit
    // dominate. Fit only if `fitOnSurface`, Clamp only if `constrainToCell`.
    // Only processed points repel (and get repelled) — rigid points are
    // ignored by the algorithms entirely.
    let active: Vec<usize> = (0..leaves.len()).filter(|&i| process[i]).collect();
    if opts.repulsion && opts.interaction_radius > 0.0 {
        relaxation_pass(&mut points, leaves, &active, opts, sdf, grad, None, skeleton);
    }

    // ── Final snap to the surface ──
    // Converge each processed point onto the SDF zero set (Fit → Clamp until
    // |sdf| < targetEpsilon or iterations) so it really finishes ON the
    // surface, within its cell — the goal of the relaxation. With repulsion
    // off this IS the pure Fit-on-Surface. Also decides "only on surface":
    // a point clamped short of the zero set never reaches epsilon and drops.
    if opts.fit_on_surface {
        for (i, leaf) in leaves.iter().enumerate() {
            if !process[i] {
                continue;
            }
            for _ in 0..opts.iterations.max(1) {
                if sdf(points[i]).abs() < eps {
                    break;
                }
                fit_step_once(&mut points[i], sdf, grad, step);
                if opts.constrain_to_cell {
                    clamp_to_cell(&mut points[i], leaf);
                }
            }
            if opts.only_on_surface && sdf(points[i]).abs() >= eps.max(1e-6) {
                alive[i] = false;
            }
        }
    }

    // ── Feature snapping (edges/corners) ──
    // AFTER the surface convergence — the sheet detection probes the normal
    // field around an ON-surface point; a point still en route would sample
    // garbage. Isotropic repulsion never lands a point on a 0D/1D feature
    // (measure zero; the tangential projection surfs around the crease), so
    // features are captured actively: detect distinct normal sheets around
    // each point, snap it onto their intersection, LOCK it (edge = 1D,
    // corner = pinned), then re-relax so edge points spread ALONG their edge
    // and free points re-equalise around the now-anchored ones.
    if opts.feature_snap {
        let cos_t = opts.feature_angle_deg.clamp(1.0, 89.0).to_radians().cos();
        let mut locks: Vec<PointLock> = vec![PointLock::Free; leaves.len()];
        let mut snapped = false;
        for &i in &active {
            if !alive[i] {
                continue;
            }
            let leaf = &leaves[i];
            let probe_r = 0.5 * leaf.size;
            // Two snap rounds = one Newton refinement on the feature: the
            // first round's anchors carry first-order probe error (FD normals
            // tilt near the crease), the re-detection at the snapped point
            // sees cleaner sheets and lands within tolerance.
            let mut kind = PointLock::Free;
            for _ in 0..2 {
                let sheets = detect_sheets(points[i], probe_r, cos_t, sdf, grad);
                let cls = match sheets.len() {
                    0 | 1 => PointLock::Free,
                    2 => PointLock::Edge,
                    _ => PointLock::Corner,
                };
                // Never DOWNGRADE: sitting exactly on the feature after round
                // 1, the probe geometry sometimes catches one sheet less —
                // re-applying the weaker snap would drag a pinned corner onto
                // an edge line. Keep the round-1 position and class instead.
                if cls == PointLock::Free || (kind == PointLock::Corner && cls == PointLock::Edge)
                {
                    break;
                }
                // Correction budget: one cell size — a genuine nearby feature
                // is never farther, anything beyond is a spurious detection.
                let Some(x) = snap_to_sheet_intersection(points[i], &sheets, leaf.size) else {
                    break;
                };
                points[i] = x;
                kind = cls;
            }
            if kind == PointLock::Free {
                continue;
            }
            locks[i] = kind;
            snapped = true;
            if opts.constrain_to_cell {
                clamp_to_cell(&mut points[i], leaf);
            }
        }
        if snapped {
            // Lock-aware smoothing round (same annealed schedule, restarted):
            // edge points slide 1D along their crease, corners stay pinned,
            // free points re-equalise around the anchors.
            if opts.repulsion && opts.interaction_radius > 0.0 {
                relaxation_pass(
                    &mut points,
                    leaves,
                    &active,
                    opts,
                    sdf,
                    grad,
                    Some((&locks, cos_t)),
                    skeleton,
                );
            }
            // Re-converge the FREE points onto the surface (the smoothing
            // round moved them; locked points sit on their feature already —
            // a fit step would drag them off the crease).
            if opts.fit_on_surface {
                for &i in &active {
                    if locks[i] != PointLock::Free || !alive[i] {
                        continue;
                    }
                    for _ in 0..opts.iterations.max(1) {
                        if sdf(points[i]).abs() < eps {
                            break;
                        }
                        fit_step_once(&mut points[i], sdf, grad, step);
                        if opts.constrain_to_cell {
                            clamp_to_cell(&mut points[i], &leaves[i]);
                        }
                    }
                }
            }
        }
    }

    (points, process, alive)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Analytic unit-sphere field (radius 10) — exact SDF + gradient.
    fn sphere_sdf(p: [f32; 3]) -> f32 {
        (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt() - 10.0
    }
    fn sphere_grad(p: [f32; 3]) -> [f32; 3] {
        let l = (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt().max(1e-9);
        [p[0] / l, p[1] / l, p[2] / l]
    }

    fn leaf_at(c: [f32; 3], size: f32) -> Leaf {
        Leaf {
            center: c,
            sdf: sphere_sdf(c),
            normal: sphere_grad(c),
            size,
        }
    }

    #[test]
    fn fit_on_surface_converges_to_the_zero_set() {
        let leaves = vec![leaf_at([9.0, 0.5, -0.5], 4.0), leaf_at([11.5, 1.0, 0.0], 4.0)];
        let opts = MesherPointOptions {
            fit_on_surface: true,
            iterations: 20,
            fit_target_epsilon: 1e-4,
            fit_step_multiplier: 0.8,
            constrain_to_cell: false,
            ..Default::default()
        };
        let (pts, process, _) = optimize_leaf_points(&leaves, &opts, &sphere_sdf, &sphere_grad, &[]);
        assert!(process.iter().all(|&b| b));
        for p in &pts {
            assert!(sphere_sdf(*p).abs() < 1e-3, "not on surface: {p:?}");
        }
    }

    #[test]
    fn constrain_to_cell_never_leaves_the_leaf_bounds() {
        // Cell far inside the sphere: the surface is out of reach, the clamp
        // must pin the point to the cell boundary instead.
        let leaf = leaf_at([2.0, 0.0, 0.0], 2.0);
        let opts = MesherPointOptions {
            fit_on_surface: true,
            iterations: 50,
            fit_target_epsilon: 1e-4,
            fit_step_multiplier: 1.0,
            constrain_to_cell: true,
            ..Default::default()
        };
        let (pts, _, _) = optimize_leaf_points(&[leaf], &opts, &sphere_sdf, &sphere_grad, &[]);
        let p = pts[0];
        for a in 0..3 {
            assert!(
                p[a] >= leaf.center[a] - 1.0 - 1e-6 && p[a] <= leaf.center[a] + 1.0 + 1e-6,
                "left the cell: {p:?}"
            );
        }
        // It pushed towards the surface as far as the cell allows (+x face).
        assert!((p[0] - 3.0).abs() < 1e-5, "not at the +x face: {p:?}");
    }

    #[test]
    fn filters_leave_unprocessed_points_rigid() {
        // One near-surface cell, one deep-inside cell; nearSurfaceOnly must
        // process only the first — the second stays at its centre.
        let near = leaf_at([9.5, 0.0, 0.0], 2.0);
        let deep = leaf_at([1.0, 0.0, 0.0], 2.0);
        let opts = MesherPointOptions {
            near_surface_only: true,
            near_surface_dist: 2.0,
            fit_on_surface: true,
            iterations: 20,
            fit_target_epsilon: 1e-4,
            constrain_to_cell: false,
            ..Default::default()
        };
        let (pts, process, _) =
            optimize_leaf_points(&[near, deep], &opts, &sphere_sdf, &sphere_grad, &[]);
        assert!(process[0] && !process[1]);
        assert!(sphere_sdf(pts[0]).abs() < 1e-3);
        assert_eq!(pts[1], deep.center, "rigid point moved");
    }

    #[test]
    fn sign_change_filter_selects_only_straddling_cells() {
        // A cell whose ±size probes straddle the surface vs. one fully inside.
        let straddling = leaf_at([9.0, 0.0, 0.0], 3.0); // probe at 12 > r → flip
        let inside = leaf_at([3.0, 0.0, 0.0], 3.0); // probes at 0..6 all inside
        let opts = MesherPointOptions {
            sign_change_neighbours_only: true,
            fit_on_surface: true,
            iterations: 20,
            fit_target_epsilon: 1e-4,
            constrain_to_cell: false,
            ..Default::default()
        };
        let (pts, process, _) =
            optimize_leaf_points(&[straddling, inside], &opts, &sphere_sdf, &sphere_grad, &[]);
        assert!(process[0], "straddling cell must be processed");
        assert!(!process[1], "interior cell must be rigid");
        assert!(sphere_sdf(pts[0]).abs() < 1e-3);
        assert_eq!(pts[1], inside.center);
    }

    /// Ticket yanv — Fit gating: the Fit op runs ONLY when its checkbox is
    /// on. With repulsion on but fit OFF, a single point (no neighbours → no
    /// repulsion force, and no fit + no final snap) stays exactly at its
    /// start; with fit ON the final snap lands it on the surface even without
    /// any repulsion force.
    #[test]
    fn fit_is_gated_by_the_fit_checkbox() {
        let start = [8.0f32, 0.0, 0.0]; // sdf = -2, off the surface
        let leaf = leaf_at(start, 20.0); // generous cell, no clamping pressure
        let base = MesherPointOptions {
            repulsion: true,
            iterations: 5,
            interaction_radius: 3.0,
            repulsion_strength: 0.5,
            fit_step_multiplier: 0.8,
            fit_target_epsilon: 1e-4,
            constrain_to_cell: false,
            ..Default::default()
        };

        // Fit OFF → no fit step, lone point, no neighbours → unchanged.
        let no_fit = MesherPointOptions { fit_on_surface: false, ..base.clone() };
        let (p_nofit, _, _) = optimize_leaf_points(&[leaf], &no_fit, &sphere_sdf, &sphere_grad, &[]);
        assert_eq!(p_nofit[0], start, "no fit + no neighbours must not move");

        // Fit ON → the phase-2 fit step pulls it onto the surface.
        let with_fit = MesherPointOptions { fit_on_surface: true, ..base };
        let (p_fit, _, _) = optimize_leaf_points(&[leaf], &with_fit, &sphere_sdf, &sphere_grad, &[]);
        assert!(
            sphere_sdf(p_fit[0]).abs() < 0.1,
            "phase-2 fit step should reach the surface: sdf={}",
            sphere_sdf(p_fit[0])
        );
    }

    #[test]
    fn repulsion_spreads_clustered_points_along_the_surface() {
        // Four points bunched together on the sphere; repulsion must increase
        // the minimum pairwise distance while keeping everyone on the surface.
        let cluster = [
            [10.0, 0.0, 0.0],
            [9.99, 0.4, 0.0],
            [9.99, 0.0, 0.4],
            [9.98, 0.3, 0.3],
        ];
        let leaves: Vec<Leaf> = cluster
            .iter()
            .map(|&c| {
                // Normalise onto the surface first, generous cell.
                let g = sphere_grad(c);
                let d = sphere_sdf(c);
                leaf_at([c[0] - d * g[0], c[1] - d * g[1], c[2] - d * g[2]], 8.0)
            })
            .collect();
        let min_dist = |pts: &[[f32; 3]]| {
            let mut m = f32::MAX;
            for i in 0..pts.len() {
                for j in (i + 1)..pts.len() {
                    let d = [
                        pts[i][0] - pts[j][0],
                        pts[i][1] - pts[j][1],
                        pts[i][2] - pts[j][2],
                    ];
                    m = m.min((d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt());
                }
            }
            m
        };
        let before = min_dist(&leaves.iter().map(|l| l.center).collect::<Vec<_>>());
        let opts = MesherPointOptions {
            repulsion: true,
            iterations: 8,
            interaction_radius: 3.0,
            repulsion_strength: 0.2,
            fit_on_surface: true,
            fit_target_epsilon: 1e-4,
            constrain_to_cell: false,
            ..Default::default()
        };
        let (pts, _, _) = optimize_leaf_points(&leaves, &opts, &sphere_sdf, &sphere_grad, &[]);
        let after = min_dist(&pts);
        assert!(
            after > before * 2.0,
            "repulsion did not spread the cluster: {before} → {after}"
        );
        for p in &pts {
            // Phase 2 does ONE fit step per repulsion iteration (ticket yanv),
            // so points stay NEAR the surface (a small fraction of the cell),
            // not exactly on it — a full re-pin is no longer performed.
            assert!(
                sphere_sdf(*p).abs() < 0.1,
                "repulsion drifted far off the surface: {p:?} (d={})",
                sphere_sdf(*p)
            );
        }
    }

    #[test]
    fn repulsion_separates_a_close_pair_on_the_surface() {
        let leaves = vec![
            leaf_at([10.0, 0.0, 0.0], 8.0),
            leaf_at([9.995, 0.3, 0.0], 8.0),
        ];
        let opts = MesherPointOptions {
            repulsion: true,
            iterations: 4,
            interaction_radius: 2.0,
            repulsion_strength: 0.1,
            // Repulsion's per-step Fit keeps the points on the surface
            // (ticket yanv: Repulsion → Fit → Clamp).
            fit_on_surface: true,
            fit_target_epsilon: 1e-4,
            constrain_to_cell: false,
            ..Default::default()
        };
        let (pts, _, _) = optimize_leaf_points(&leaves, &opts, &sphere_sdf, &sphere_grad, &[]);
        let d = [
            pts[0][0] - pts[1][0],
            pts[0][1] - pts[1][1],
            pts[0][2] - pts[1][2],
        ];
        let dist = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        assert!(dist > 0.3, "points did not separate: {dist}");
        for p in &pts {
            assert!(sphere_sdf(*p).abs() < 0.1);
        }
    }

    // ── Feature snapping (edges/corners) ──

    /// Exact Euclidean box SDF, half-extent `h`, centred at origin.
    fn box_sdf(p: [f32; 3], h: f32) -> f32 {
        let q = [p[0].abs() - h, p[1].abs() - h, p[2].abs() - h];
        let outside = [q[0].max(0.0), q[1].max(0.0), q[2].max(0.0)];
        let out_len =
            (outside[0] * outside[0] + outside[1] * outside[1] + outside[2] * outside[2]).sqrt();
        out_len + q[0].max(q[1]).max(q[2]).min(0.0)
    }
    fn box5_sdf(p: [f32; 3]) -> f32 {
        box_sdf(p, 5.0)
    }
    /// Central-difference gradient of the box field (the probes sample the
    /// smooth sheets away from the crease, so FD is fine).
    fn box5_grad(p: [f32; 3]) -> [f32; 3] {
        let e = 1e-3;
        let g = [
            box5_sdf([p[0] + e, p[1], p[2]]) - box5_sdf([p[0] - e, p[1], p[2]]),
            box5_sdf([p[0], p[1] + e, p[2]]) - box5_sdf([p[0], p[1] - e, p[2]]),
            box5_sdf([p[0], p[1], p[2] + e]) - box5_sdf([p[0], p[1], p[2] - e]),
        ];
        let l = (g[0] * g[0] + g[1] * g[1] + g[2] * g[2]).sqrt().max(1e-12);
        [g[0] / l, g[1] / l, g[2] / l]
    }
    fn box_leaf_at(c: [f32; 3], size: f32) -> Leaf {
        Leaf { center: c, sdf: box5_sdf(c), normal: box5_grad(c), size }
    }

    /// Points fitted near a box edge / corner must SNAP exactly onto them:
    /// two coordinates at ±5 for the edge point, all three for the corner —
    /// the anti-chamfer guarantee isotropic repulsion can never give.
    #[test]
    fn feature_snap_lands_points_on_box_edges_and_corners() {
        let near_edge = box_leaf_at([4.4, 4.4, 0.3], 2.0); // near edge x=y=5
        let near_corner = box_leaf_at([4.4, 4.4, 4.4], 2.0); // near corner (5,5,5)
        let mid_face = box_leaf_at([0.5, 0.3, 4.2], 2.0); // plain face point
        let opts = MesherPointOptions {
            fit_on_surface: true,
            iterations: 20,
            fit_target_epsilon: 1e-4,
            constrain_to_cell: false,
            feature_snap: true,
            feature_angle_deg: 30.0,
            ..Default::default()
        };
        let (pts, _, _) =
            optimize_leaf_points(&[near_edge, near_corner, mid_face], &opts, &box5_sdf, &box5_grad, &[]);

        let tol = 0.05;
        // Edge point: x and y pinned to the crease, z stays interior.
        assert!(
            (pts[0][0] - 5.0).abs() < tol && (pts[0][1] - 5.0).abs() < tol,
            "edge point not on the crease: {:?}",
            pts[0]
        );
        assert!(pts[0][2].abs() < 4.0, "edge point escaped along the edge: {:?}", pts[0]);
        // Corner point: all three coordinates at +5.
        for a in 0..3 {
            assert!(
                (pts[1][a] - 5.0).abs() < tol,
                "corner point not on the corner: {:?}",
                pts[1]
            );
        }
        // Face point: NOT snapped anywhere, just on the surface (z = 5).
        assert!((pts[2][2] - 5.0).abs() < 1e-3, "face point off surface: {:?}", pts[2]);
        assert!(pts[2][0].abs() < 2.0 && pts[2][1].abs() < 2.0, "face point dragged: {:?}", pts[2]);
    }

    /// A smooth surface has no features — enabling the snap must not move
    /// anything beyond what the plain pipeline does.
    #[test]
    fn feature_snap_is_inert_on_a_smooth_sphere() {
        let leaves = vec![leaf_at([9.2, 0.5, -0.5], 4.0), leaf_at([0.3, 9.4, 0.4], 4.0)];
        let base = MesherPointOptions {
            fit_on_surface: true,
            iterations: 20,
            fit_target_epsilon: 1e-4,
            constrain_to_cell: false,
            ..Default::default()
        };
        let with_snap = MesherPointOptions {
            feature_snap: true,
            feature_angle_deg: 30.0,
            ..base.clone()
        };
        let (plain, _, _) = optimize_leaf_points(&leaves, &base, &sphere_sdf, &sphere_grad, &[]);
        let (snapped, _, _) = optimize_leaf_points(&leaves, &with_snap, &sphere_sdf, &sphere_grad, &[]);
        for (a, b) in plain.iter().zip(&snapped) {
            let d = [(a[0] - b[0]), (a[1] - b[1]), (a[2] - b[2])];
            let dist = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
            assert!(dist < 1e-4, "snap moved a smooth-surface point: {a:?} -> {b:?}");
        }
    }

    /// With repulsion on, edge-locked points spread ALONG the crease (1D)
    /// while staying on it, and a corner point stays pinned.
    #[test]
    fn edge_locked_points_spread_along_the_crease() {
        // Three points bunched on the edge x=y=5, plus the corner (5,5,5).
        let leaves = vec![
            box_leaf_at([4.5, 4.5, -0.4], 3.0),
            box_leaf_at([4.5, 4.5, 0.0], 3.0),
            box_leaf_at([4.5, 4.5, 0.4], 3.0),
            box_leaf_at([4.6, 4.6, 4.6], 3.0),
        ];
        let opts = MesherPointOptions {
            fit_on_surface: true,
            iterations: 12,
            fit_target_epsilon: 1e-4,
            constrain_to_cell: false,
            repulsion: true,
            interaction_radius: 3.0,
            repulsion_strength: 0.5,
            repulsion_damping: 0.2,
            feature_snap: true,
            feature_angle_deg: 30.0,
            ..Default::default()
        };
        let (pts, _, _) = optimize_leaf_points(&leaves, &opts, &box5_sdf, &box5_grad, &[]);
        let tol = 0.08;
        // All three edge points still ON the crease…
        for p in &pts[..3] {
            assert!(
                (p[0] - 5.0).abs() < tol && (p[1] - 5.0).abs() < tol,
                "edge point left the crease: {p:?}"
            );
        }
        // …but spread apart along z compared to the 0.4 start spacing.
        let mut zs: Vec<f32> = pts[..3].iter().map(|p| p[2]).collect();
        zs.sort_by(f32::total_cmp);
        assert!(
            zs[1] - zs[0] > 0.45 && zs[2] - zs[1] > 0.45,
            "edge points did not spread along the edge: {zs:?}"
        );
        // Corner point pinned at (5,5,5).
        for a in 0..3 {
            assert!((pts[3][a] - 5.0).abs() < tol, "corner point moved: {:?}", pts[3]);
        }
    }

    // ── B-Rep skeleton (Hybrid-PASM P1, ticket nwmi) ──

    #[test]
    fn resample_polyline_uniform_spacing_exact_endpoints() {
        // Straight 10mm line at h=1 → 11 points, exact ends, ~1mm spacing.
        let line = vec![[0.0f32, 0.0, 0.0], [4.0, 0.0, 0.0], [10.0, 0.0, 0.0]];
        let r = resample_polyline(&line, 1.0);
        assert_eq!(r.len(), 11);
        assert_eq!(r[0], [0.0, 0.0, 0.0]);
        assert_eq!(*r.last().unwrap(), [10.0, 0.0, 0.0]);
        for w in r.windows(2) {
            let d = (w[1][0] - w[0][0]).abs();
            assert!((d - 1.0).abs() < 1e-4, "spacing {d}");
        }
        // Coarser than the chain → endpoints only (plus midpoint at h=5).
        let r2 = resample_polyline(&line, 20.0);
        assert_eq!(r2.len(), 2);
    }

    /// Fixed skeleton anchors repel free particles but never move: a free
    /// point squeezed between two anchors on the sphere slides away along the
    /// surface; the anchors stay bit-identical.
    #[test]
    fn skeleton_anchors_repel_but_never_move() {
        let g = sphere_grad([10.0, 0.0, 0.0]);
        let _ = g;
        let free = leaf_at([9.9, 0.05, 0.0], 6.0);
        let anchors = [[10.0f32, 0.0, 0.0]];
        let opts = MesherPointOptions {
            repulsion: true,
            iterations: 10,
            interaction_radius: 3.0,
            repulsion_strength: 0.3,
            fit_on_surface: true,
            fit_target_epsilon: 1e-4,
            constrain_to_cell: false,
            ..Default::default()
        };
        let (pts, _, _) =
            optimize_leaf_points(&[free], &opts, &sphere_sdf, &sphere_grad, &anchors);
        // Contract: the anchor repels — the free point (0.11 away at start)
        // must be pushed clearly off while staying on the surface. The anchor
        // itself is immutable by construction (a shared slice).
        let d = |a: [f32; 3], b: [f32; 3]| {
            ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
        };
        let start = [9.9f32, 0.05, 0.0];
        let before = d(start, anchors[0]);
        let after = d(pts[0], anchors[0]);
        assert!(
            after > before + 0.2,
            "anchor did not repel: {before} -> {after}, p={:?}",
            pts[0]
        );
        assert!(sphere_sdf(pts[0]).abs() < 0.1, "left the surface: {:?}", pts[0]);
    }

    /// Ticket l45r: a tight clump with a HUGE strength (the reported
    /// strength=100). The relaxation must stay STABLE — spread the clump
    /// toward even spacing while keeping every point in its local
    /// neighbourhood. Without per-step displacement limiting the stiff 1/d²
    /// force ejects points off the surface (they leap across the sphere),
    /// which in the app reads as the cluster "collapsing into the corner"
    /// once only-on-surface drops the ejected ones.
    #[test]
    fn strong_repulsion_stays_local_and_spreads_without_ejecting() {
        let mut clump = Vec::new();
        for iy in 0..4 {
            for iz in 0..4 {
                let c = [10.0, (iy as f32 - 1.5) * 0.2, (iz as f32 - 1.5) * 0.2];
                let g = sphere_grad(c);
                let d = sphere_sdf(c);
                clump.push([c[0] - d * g[0], c[1] - d * g[1], c[2] - d * g[2]]);
            }
        }
        let leaves: Vec<Leaf> = clump.iter().map(|&c| leaf_at(c, 8.0)).collect();
        let pair = |pts: &[[f32; 3]], want_min: bool| {
            let mut m = if want_min { f32::MAX } else { 0.0 };
            for i in 0..pts.len() {
                for j in (i + 1)..pts.len() {
                    let d = [pts[i][0] - pts[j][0], pts[i][1] - pts[j][1], pts[i][2] - pts[j][2]];
                    let dl = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
                    m = if want_min { m.min(dl) } else { m.max(dl) };
                }
            }
            m
        };
        let starts: Vec<[f32; 3]> = leaves.iter().map(|l| l.center).collect();
        let before_min = pair(&starts, true);
        let opts = MesherPointOptions {
            repulsion: true,
            iterations: 10,
            interaction_radius: 3.0,
            repulsion_strength: 100.0,
            fit_on_surface: true,
            fit_target_epsilon: 1e-4,
            constrain_to_cell: false,
            ..Default::default()
        };
        let (pts, _, _) = optimize_leaf_points(&leaves, &opts, &sphere_sdf, &sphere_grad, &[]);
        assert!(
            pair(&pts, true) > before_min,
            "clump did not spread: min {before_min} -> {}",
            pair(&pts, true)
        );
        // A stable, local relaxation keeps the cluster compact; an unstable
        // blow-up + refit scatters points ~a sphere diameter apart.
        assert!(
            pair(&pts, false) < 4.0,
            "points ejected/scattered across the surface: max pairwise {}",
            pair(&pts, false)
        );
        for p in &pts {
            assert!(sphere_sdf(*p).abs() < 0.2, "drifted off surface: {p:?} d={}", sphere_sdf(*p));
        }
    }

    // ── Octree / balance / dual-contour meshing ──

    fn sphere_octree(max_depth: u32) -> OctreeNode {
        build_octree([0.0; 3], 32.0, max_depth, 1.0, &sphere_sdf)
    }

    /// Depth of the leaf containing p (test helper mirroring leaf_depth_at).
    fn depth_at(root: &OctreeNode, p: [f32; 3]) -> u32 {
        let mut n = root;
        loop {
            match &n.children {
                None => return n.depth,
                Some(ch) => {
                    let i = (p[0] > n.center[0]) as usize
                        + 2 * ((p[1] > n.center[1]) as usize)
                        + 4 * ((p[2] > n.center[2]) as usize);
                    n = &ch[i];
                }
            }
        }
    }

    fn leaves_of(root: &OctreeNode) -> Vec<(&OctreeNode, ())> {
        let mut out = Vec::new();
        let mut stack = vec![root];
        while let Some(n) = stack.pop() {
            match &n.children {
                Some(ch) => stack.extend(ch.iter()),
                None => out.push((n, ())),
            }
        }
        out
    }

    #[test]
    fn balance_enforces_the_two_to_one_rule() {
        let mut root = sphere_octree(6);
        balance_2to1(&mut root, &sphere_sdf);
        for (leaf, _) in leaves_of(&root) {
            let eps = leaf.size * 0.01;
            for axis in 0..3 {
                for dir in [-1.0f32, 1.0] {
                    let mut q = leaf.center;
                    q[axis] += dir * (leaf.size * 0.5 + eps);
                    if q[axis].abs() > 16.0 {
                        continue; // outside the root cube
                    }
                    let nd = depth_at(&root, q);
                    assert!(
                        nd + 1 >= leaf.depth,
                        "2:1 violated: leaf depth {} next to depth {nd}",
                        leaf.depth
                    );
                }
            }
        }
    }

    /// The octree-edge connectivity mesher connects ONLY shown leaves and
    /// makes no watertightness claim (holes/wrong facets are expected). Here
    /// the near-surface leaves are shown: adjacent ones must connect, and no
    /// triangle may reference a hidden leaf.
    #[test]
    fn octree_connect_uses_only_shown_leaves() {
        let mut root = sphere_octree(5);
        balance_2to1(&mut root, &sphere_sdf);
        let leaves = collect_leaves(&mut root, &sphere_grad);
        let shown: Vec<bool> = leaves.iter().map(|l| l.sdf.abs() < 3.0).collect();
        let indices = connect_octree_edges(&root, &shown);
        assert!(!indices.is_empty(), "adjacent shown leaves must connect");
        for &i in &indices {
            assert!(shown[i as usize], "triangle referenced a hidden leaf");
        }
        // Every triangle is non-degenerate (3 distinct leaf vertices).
        for t in indices.chunks(3) {
            assert!(t[0] != t[1] && t[1] != t[2] && t[2] != t[0], "degenerate tri");
        }
    }

    /// "Only on surface": a cell whose point cannot reach the zero set
    /// (clamped at its bounds) drops OUT of the set; a reaching cell stays.
    #[test]
    fn only_on_surface_drops_clamped_points() {
        let reaching = leaf_at([9.5, 0.0, 0.0], 2.0); // surface within bounds
        let deep = leaf_at([2.0, 0.0, 0.0], 2.0); // surface out of reach
        let opts = MesherPointOptions {
            fit_on_surface: true,
            only_on_surface: true,
            iterations: 30,
            fit_target_epsilon: 1e-4,
            fit_step_multiplier: 1.0,
            constrain_to_cell: true,
            ..Default::default()
        };
        let (pts, _, alive) =
            optimize_leaf_points(&[reaching, deep], &opts, &sphere_sdf, &sphere_grad, &[]);
        assert!(alive[0], "reaching point must survive");
        assert!(!alive[1], "clamped point must drop out");
        assert!(sphere_sdf(pts[0]).abs() < 1e-3);
    }

    /// No shown leaves → no triangles (the mesh spans only displayed points).
    #[test]
    fn octree_connect_empty_without_shown_leaves() {
        let mut root = sphere_octree(4);
        let leaves = collect_leaves(&mut root, &sphere_grad);
        let shown = vec![false; leaves.len()];
        assert!(connect_octree_edges(&root, &shown).is_empty());
    }
}

// ═══════════════════════════════════════════════════════════════════
// Adaptive octree + 2:1 balancing + dual-contour meshing over the
// optimised points (jq2t follow-up: "mesh the filtered/projected/
// repelled points, one per leaf cell").
// ═══════════════════════════════════════════════════════════════════

/// Explicit adaptive octree. The debug pipeline needs the TREE (not just
/// the leaf list) twice: the 2:1 balance pass splits coarse leaves next to
/// much finer ones, and the dual-contour polygonisation is the classic
/// recursive cellProc/faceProc/edgeProc walk over the hierarchy.
pub(crate) struct OctreeNode {
    pub center: [f32; 3],
    pub size: f32,
    pub depth: u32,
    /// Centre SDF sample (kept for leaves).
    pub sdf: f32,
    pub children: Option<Box<[OctreeNode; 8]>>,
    /// Index into the collected leaf list (usize::MAX until collected).
    pub leaf_idx: usize,
}

impl OctreeNode {
    fn is_leaf(&self) -> bool {
        self.children.is_none()
    }
}

const MAX_LEAVES: usize = 200_000;
const SQRT3: f32 = 1.732_050_8;

/// Child index convention: `c(sx, sy, sz) = sx + 2·sy + 4·sz`, where 0 is
/// the lower and 1 the upper half along each axis.
fn child_center(c: [f32; 3], size: f32, i: usize) -> [f32; 3] {
    let h = size * 0.25;
    [
        c[0] + if i & 1 != 0 { h } else { -h },
        c[1] + if i & 2 != 0 { h } else { -h },
        c[2] + if i & 4 != 0 { h } else { -h },
    ]
}

fn split_node(node: &mut OctreeNode, sdf: &dyn Fn([f32; 3]) -> f32) {
    let mut make = |i: usize| {
        let cc = child_center(node.center, node.size, i);
        OctreeNode {
            center: cc,
            size: node.size * 0.5,
            depth: node.depth + 1,
            sdf: sdf(cc),
            children: None,
            leaf_idx: usize::MAX,
        }
    };
    node.children = Some(Box::new([
        make(0),
        make(1),
        make(2),
        make(3),
        make(4),
        make(5),
        make(6),
        make(7),
    ]));
}

/// Build the narrow-band adaptive octree (same band criterion as the
/// original flat sampler): a cell subdivides while its centre lies within
/// `|sdf| ≤ halfDiagonal · guard` and `depth < max_depth`.
pub(crate) fn build_octree(
    root_center: [f32; 3],
    root_size: f32,
    max_depth: u32,
    band_guard: f32,
    sdf: &dyn Fn([f32; 3]) -> f32,
) -> OctreeNode {
    let guard = band_guard.max(0.1);
    let max_depth = max_depth.min(10);
    let mut root = OctreeNode {
        center: root_center,
        size: root_size,
        depth: 0,
        sdf: sdf(root_center),
        children: None,
        leaf_idx: usize::MAX,
    };
    let mut leaves = 1usize;
    // Depth-first subdivision with a global leaf budget.
    let mut stack: Vec<*mut OctreeNode> = vec![&mut root];
    while let Some(ptr) = stack.pop() {
        // SAFETY: nodes are only pushed once and the arena (the tree) lives
        // for the whole loop; children boxes are never moved after creation.
        let node = unsafe { &mut *ptr };
        let half_diag = node.size * 0.5 * SQRT3;
        let in_band = node.sdf.abs() <= half_diag * guard;
        if node.depth < max_depth && in_band && leaves + 7 <= MAX_LEAVES {
            split_node(node, sdf);
            leaves += 7;
            for ch in node.children.as_mut().unwrap().iter_mut() {
                stack.push(ch);
            }
        }
    }
    root
}

/// Depth of the leaf containing `p` (root cube assumed to contain `p`).
fn leaf_depth_at(root: &OctreeNode, p: [f32; 3]) -> u32 {
    let mut n = root;
    loop {
        match &n.children {
            None => return n.depth,
            Some(ch) => {
                let i = (p[0] > n.center[0]) as usize
                    + 2 * ((p[1] > n.center[1]) as usize)
                    + 4 * ((p[2] > n.center[2]) as usize);
                n = &ch[i];
            }
        }
    }
}

/// Centre of the leaf containing `p` (dedup key for the balance pass).
fn leaf_center_at(root: &OctreeNode, p: [f32; 3]) -> [f32; 3] {
    let mut n = root;
    loop {
        match &n.children {
            None => return n.center,
            Some(ch) => {
                let i = (p[0] > n.center[0]) as usize
                    + 2 * ((p[1] > n.center[1]) as usize)
                    + 4 * ((p[2] > n.center[2]) as usize);
                n = &ch[i];
            }
        }
    }
}

/// Split the leaf containing `p` (no-op if it is already subdivided).
fn split_leaf_at(root: &mut OctreeNode, p: [f32; 3], sdf: &dyn Fn([f32; 3]) -> f32) {
    let mut n = root;
    loop {
        if n.is_leaf() {
            split_node(n, sdf);
            return;
        }
        let i = (p[0] > n.center[0]) as usize
            + 2 * ((p[1] > n.center[1]) as usize)
            + 4 * ((p[2] > n.center[2]) as usize);
        n = &mut n.children.as_mut().unwrap()[i];
    }
}

/// Enforce the 2:1 rule: face-adjacent leaves may differ by at most one
/// depth level. Deep leaves probe just beyond their 6 faces; a much
/// coarser neighbour gets split. Repeats until stable (bounded — every
/// split only raises a neighbour towards `own depth − 1`, and the leaf
/// budget caps growth).
pub(crate) fn balance_2to1(root: &mut OctreeNode, sdf: &dyn Fn([f32; 3]) -> f32) {
    for _ in 0..32 {
        // Collect (probe point, required minimum depth) for violations.
        let mut splits: Vec<[f32; 3]> = Vec::new();
        let mut count = 0usize;
        let mut stack: Vec<&OctreeNode> = vec![root];
        let mut leaves: Vec<(&OctreeNode, ())> = Vec::new();
        while let Some(n) = stack.pop() {
            match &n.children {
                Some(ch) => stack.extend(ch.iter()),
                None => {
                    count += 1;
                    leaves.push((n, ()));
                }
            }
        }
        let root_ref: &OctreeNode = root;
        for (leaf, _) in &leaves {
            let eps = leaf.size * 0.01;
            for axis in 0..3 {
                for dir in [-1.0f32, 1.0] {
                    let mut q = leaf.center;
                    q[axis] += dir * (leaf.size * 0.5 + eps);
                    // Outside the root cube → no neighbour.
                    let h = root_ref.size * 0.5;
                    if (q[0] - root_ref.center[0]).abs() > h
                        || (q[1] - root_ref.center[1]).abs() > h
                        || (q[2] - root_ref.center[2]).abs() > h
                    {
                        continue;
                    }
                    if leaf_depth_at(root_ref, q) + 1 < leaf.depth {
                        splits.push(q);
                    }
                }
            }
        }
        if splits.is_empty() {
            return;
        }
        // Many deep leaves report the SAME coarse neighbour (once per probe)
        // — deduplicate by the target leaf's centre, or the leaf-budget
        // guard trips on phantom volume and balancing never runs.
        let mut seen: std::collections::HashSet<(u32, u32, u32)> =
            std::collections::HashSet::new();
        let mut unique: Vec<[f32; 3]> = Vec::new();
        for q in splits {
            let c = leaf_center_at(root_ref, q);
            let key = (c[0].to_bits(), c[1].to_bits(), c[2].to_bits());
            if seen.insert(key) {
                unique.push(q);
            }
        }
        if count + unique.len() * 7 > MAX_LEAVES {
            return;
        }
        for q in unique {
            split_leaf_at(root, q, sdf);
        }
    }
}

/// Collect the leaves into a flat list (assigning `leaf_idx`), with centre
/// gradient for the point pipeline.
pub(crate) fn collect_leaves(
    root: &mut OctreeNode,
    grad: &dyn Fn([f32; 3]) -> [f32; 3],
) -> Vec<Leaf> {
    let mut out = Vec::new();
    let mut stack: Vec<&mut OctreeNode> = vec![root];
    while let Some(n) = stack.pop() {
        match &mut n.children {
            Some(ch) => stack.extend(ch.iter_mut()),
            None => {
                n.leaf_idx = out.len();
                out.push(Leaf {
                    center: n.center,
                    sdf: n.sdf,
                    normal: grad(n.center),
                    size: n.size,
                });
            }
        }
    }
    out
}

// ── Octree-edge connectivity mesher ─────────────────────────────────
//
// NOT dual contouring. One vertex per leaf (the filtered/optimised point);
// for every MINIMAL octree edge, the (up to) 4 leaves sharing that edge are
// connected into a quad (two triangles) — PURELY by octree adjacency, with
// NO sign-change test. Only edges whose 4 leaves are all "shown" (survived
// the display filters) contribute, so the mesh is built over exactly the
// displayed points. It deliberately makes no watertightness/orientation
// guarantees (holes and wrong facets are expected) — it is a raw
// visualisation of the octree edge graph over the filtered point set, drawn
// as a wireframe. The recursion (`cell_proc`/`face_proc`/`edge_proc`) is the
// standard adaptive-octree minimal-edge enumeration.

struct MeshCtx<'a> {
    /// Per-leaf display flag (= passed the filters). A quad is emitted only
    /// when all 4 of its leaves are shown, so the mesh spans exactly the
    /// displayed points.
    shown: &'a [bool],
    indices: Vec<u32>,
}

/// For each of the 4 nodes around an edge along `axis` (cyclic order), the
/// child pair adjacent to the shared edge, per segment s ∈ {lower, upper}.
/// Cyclic node order (positions of the cells around the edge, in the two
/// axes perpendicular to `axis`, using (u,v) = the next two axes):
///   node0 at (u−,v−), node1 at (u+,v−), node2 at (u+,v+), node3 at (u−,v+)
/// so node i touches the edge with its child at the OPPOSITE (u,v) corner.
fn edge_child(axis: usize, node_pos: usize, seg: usize) -> usize {
    // Child (su, sv) facing the shared edge for each cyclic position.
    let (su, sv) = match node_pos {
        0 => (1, 1),
        1 => (0, 1),
        2 => (0, 0),
        _ => (1, 0),
    };
    // Map (axis, u, v) back to (sx, sy, sz); u,v are the axes after `axis`.
    let (u_axis, v_axis) = ((axis + 1) % 3, (axis + 2) % 3);
    let mut s = [0usize; 3];
    s[axis] = seg;
    s[u_axis] = su;
    s[v_axis] = sv;
    s[0] + 2 * s[1] + 4 * s[2]
}

fn node_or_child<'a>(n: &'a OctreeNode, idx: usize) -> &'a OctreeNode {
    match &n.children {
        None => n,
        Some(ch) => &ch[idx],
    }
}

fn cell_proc(n: &OctreeNode, ctx: &mut MeshCtx) {
    let Some(ch) = &n.children else { return };
    for c in ch.iter() {
        cell_proc(c, ctx);
    }
    // 12 interior faces between sibling pairs.
    for axis in 0..3 {
        let step = 1usize << axis;
        for a in 0..8usize {
            if a & step == 0 {
                face_proc(&ch[a], &ch[a + step], axis, ctx);
            }
        }
    }
    // 6 interior edge segments (2 per axis).
    for axis in 0..3 {
        let (u_axis, v_axis) = ((axis + 1) % 3, (axis + 2) % 3);
        for seg in 0..2usize {
            let mut quad: [&OctreeNode; 4] = [&ch[0]; 4];
            for (pos, q) in quad.iter_mut().enumerate() {
                let (su, sv) = match pos {
                    0 => (0, 0),
                    1 => (1, 0),
                    2 => (1, 1),
                    _ => (0, 1),
                };
                let mut s = [0usize; 3];
                s[axis] = seg;
                s[u_axis] = su;
                s[v_axis] = sv;
                *q = &ch[s[0] + 2 * s[1] + 4 * s[2]];
            }
            edge_proc(quad, axis, ctx);
        }
    }
}

fn face_proc(a: &OctreeNode, b: &OctreeNode, axis: usize, ctx: &mut MeshCtx) {
    if a.is_leaf() && b.is_leaf() {
        return;
    }
    let (u_axis, v_axis) = ((axis + 1) % 3, (axis + 2) % 3);
    // 4 face-adjacent child pairs across the shared face.
    for su in 0..2usize {
        for sv in 0..2usize {
            let mut sa = [0usize; 3];
            sa[axis] = 1; // upper child of the lower node
            sa[u_axis] = su;
            sa[v_axis] = sv;
            let mut sb = sa;
            sb[axis] = 0; // lower child of the upper node
            face_proc(
                node_or_child(a, sa[0] + 2 * sa[1] + 4 * sa[2]),
                node_or_child(b, sb[0] + 2 * sb[1] + 4 * sb[2]),
                axis,
                ctx,
            );
        }
    }
    // 4 edges lying in the shared face: 2 parallel to u, 2 parallel to v.
    // An edge parallel to `u` at segment su: its cyclic (u', v') axes are
    // (u_axis+1, u_axis+2) mod 3 — one of them is `axis`. Assemble the 4
    // cells around it from a (its upper-axis children) and b (lower).
    for (edge_axis, seg_axis) in [(u_axis, u_axis), (v_axis, v_axis)] {
        let _ = seg_axis;
        for seg in 0..2usize {
            let mut quad: [&OctreeNode; 4] = [a; 4];
            let (eu, ev) = ((edge_axis + 1) % 3, (edge_axis + 2) % 3);
            for (pos, q) in quad.iter_mut().enumerate() {
                let (su, sv) = match pos {
                    0 => (0, 0),
                    1 => (1, 0),
                    2 => (1, 1),
                    _ => (0, 1),
                };
                let mut s = [0usize; 3];
                s[edge_axis] = seg;
                s[eu] = su;
                s[ev] = sv;
                // Along `axis` the quad spans a (s[axis]=0 side) and b
                // (s[axis]=1 side): pick the node and force its child
                // coordinate on that axis to face the shared plane.
                let from_b = s[axis] == 1;
                let mut cs = s;
                cs[axis] = if from_b { 0 } else { 1 };
                let n = if from_b { b } else { a };
                *q = node_or_child(n, cs[0] + 2 * cs[1] + 4 * cs[2]);
            }
            edge_proc(quad, edge_axis, ctx);
        }
    }
}

fn edge_proc(nodes: [&OctreeNode; 4], axis: usize, ctx: &mut MeshCtx) {
    if nodes.iter().all(|n| n.is_leaf()) {
        emit_edge_quad(nodes, axis, ctx);
        return;
    }
    for seg in 0..2usize {
        let mut sub: [&OctreeNode; 4] = nodes;
        for (pos, q) in sub.iter_mut().enumerate() {
            *q = node_or_child(nodes[pos], edge_child(axis, pos, seg));
        }
        edge_proc(sub, axis, ctx);
    }
}

/// Connect the 4 leaves around a minimal octree edge into a quad (two
/// triangles), by pure adjacency — no sign-change test. Emitted only when
/// every one of the 4 leaves is shown, so the mesh spans exactly the
/// displayed points. Winding is irrelevant (drawn as a wireframe);
/// degenerate triangles (a coarse leaf spanning two quadrants makes cells
/// coincide) are skipped.
fn emit_edge_quad(nodes: [&OctreeNode; 4], _axis: usize, ctx: &mut MeshCtx) {
    if nodes.iter().any(|n| !ctx.shown[n.leaf_idx]) {
        return;
    }
    let idx = [
        nodes[0].leaf_idx as u32,
        nodes[1].leaf_idx as u32,
        nodes[2].leaf_idx as u32,
        nodes[3].leaf_idx as u32,
    ];
    let tris = [[idx[0], idx[1], idx[2]], [idx[0], idx[2], idx[3]]];
    for t in tris {
        if t[0] != t[1] && t[1] != t[2] && t[2] != t[0] {
            ctx.indices.extend_from_slice(&t);
        }
    }
}

/// Connect the (optionally balanced) octree's minimal-edge neighbourhoods
/// over the shown leaves into triangles (see the module note). Returns
/// triangle indices into the leaf/point list.
pub(crate) fn connect_octree_edges(root: &OctreeNode, shown: &[bool]) -> Vec<u32> {
    let mut ctx = MeshCtx {
        shown,
        indices: Vec::new(),
    };
    cell_proc(root, &mut ctx);
    ctx.indices
}
