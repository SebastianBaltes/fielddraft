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
//!    surface tangent plane).
//!
//! ONE interleaved relaxation loop (ticket yanv): per iteration
//! **Repulsion → Fit → Clamp**, for `repulsionIterations`, with the repulsion
//! strength ANNEALED from full down to ~0 across the iterations — early steps
//! spread aggressively, the fit dominates at the end. A final **Fit → Clamp**
//! convergence (to `targetEpsilon` / `fitMaxIterations`) then snaps each point
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
    // Feature 1: Newton projection.
    pub fit_on_surface: bool,
    /// Drop points that did NOT actually reach the surface after the fit
    /// (typically clamped at their cell bounds): they leave the point set
    /// entirely — not drawn, not repelled, their cells emit no mesh vertex.
    pub only_on_surface: bool,
    pub fit_max_iterations: u32,
    pub fit_target_epsilon: f32,
    pub fit_step_multiplier: f32,
    // Feature 2: tangential relaxation.
    pub repulsion: bool,
    pub repulsion_iterations: u32,
    pub interaction_radius: f32,
    pub repulsion_strength: f32,
    pub weighting_function: WeightingFunction,
    // Meshing over the optimised points (adaptive dual contouring).
    pub mesh_points: bool,
    /// Enforce the 2:1 octree balance before meshing (bounded depth
    /// contrast between face neighbours → bounded triangle quality).
    pub balance_octree: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WeightingFunction {
    Linear,
    InverseSquare,
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
            fit_on_surface: false,
            only_on_surface: false,
            fit_max_iterations: 8,
            fit_target_epsilon: 0.001,
            fit_step_multiplier: 0.8,
            repulsion: false,
            repulsion_iterations: 4,
            interaction_radius: 2.0,
            repulsion_strength: 0.5,
            weighting_function: WeightingFunction::Linear,
            mesh_points: false,
            balance_octree: true,
        }
    }
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

/// Run the jq2t point pipeline over the leaves. Returns the final position of
/// every leaf's point (unprocessed points stay at the cell centre) plus which
/// ones were processed. `sdf`/`grad` evaluate the scene field.
pub(crate) fn optimize_leaf_points(
    leaves: &[Leaf],
    opts: &MesherPointOptions,
    sdf: &dyn Fn([f32; 3]) -> f32,
    grad: &dyn Fn([f32; 3]) -> [f32; 3],
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
    if opts.repulsion && opts.interaction_radius > 0.0 {
        let r = opts.interaction_radius;
        let inv_cell = 1.0 / r;
        let key = |p: &[f32; 3]| -> (i32, i32, i32) {
            (
                (p[0] * inv_cell).floor() as i32,
                (p[1] * inv_cell).floor() as i32,
                (p[2] * inv_cell).floor() as i32,
            )
        };
        // Only processed points repel (and get repelled) — rigid points are
        // ignored by the algorithms entirely.
        let active: Vec<usize> = (0..leaves.len()).filter(|&i| process[i]).collect();
        let iters = opts.repulsion_iterations.max(1);
        for t in 0..iters {
            // Anneal: repulsion is strongest first, fading linearly toward 0.
            let anneal = 1.0 - t as f32 / iters as f32;
            let strength = opts.repulsion_strength * anneal;
            // Grid hash over the CURRENT positions of the active points.
            let mut grid: std::collections::HashMap<(i32, i32, i32), Vec<usize>> =
                std::collections::HashMap::new();
            for &i in &active {
                grid.entry(key(&points[i])).or_default().push(i);
            }
            let mut moved = points.clone();
            for &i in &active {
                let p = points[i];
                let (kx, ky, kz) = key(&p);
                let mut f = [0.0f32; 3];
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
                                let q = points[j];
                                let dvec = [p[0] - q[0], p[1] - q[1], p[2] - q[2]];
                                let dist =
                                    (dvec[0] * dvec[0] + dvec[1] * dvec[1] + dvec[2] * dvec[2])
                                        .sqrt();
                                if dist >= r || dist < 1e-9 {
                                    continue;
                                }
                                let w = match opts.weighting_function {
                                    WeightingFunction::Linear => 1.0 - dist / r,
                                    // Normalised so w→1 at dist=r; capped so
                                    // near-coincident points can't explode.
                                    WeightingFunction::InverseSquare => {
                                        ((r / dist) * (r / dist)).min(100.0)
                                    }
                                };
                                let s = w / dist;
                                f[0] += dvec[0] * s;
                                f[1] += dvec[1] * s;
                                f[2] += dvec[2] * s;
                            }
                        }
                    }
                }
                let leaf = &leaves[i];
                let mut np = p;
                // Repulsion: tangential component of the neighbour force,
                // scaled by the annealed strength (stays on the surface plane).
                if f != [0.0, 0.0, 0.0] {
                    let n = grad(p);
                    let fn_dot = f[0] * n[0] + f[1] * n[1] + f[2] * n[2];
                    let ft = [f[0] - fn_dot * n[0], f[1] - fn_dot * n[1], f[2] - fn_dot * n[2]];
                    np = [p[0] + ft[0] * strength, p[1] + ft[1] * strength, p[2] + ft[2] * strength];
                }
                // Fit (single Newton step): nudge back onto the surface after
                // the tangential move.
                if opts.fit_on_surface {
                    fit_step_once(&mut np, sdf, grad, step);
                }
                if opts.constrain_to_cell {
                    clamp_to_cell(&mut np, leaf);
                }
                moved[i] = np;
            }
            points = moved;
        }
    }

    // ── Final snap to the surface ──
    // Converge each processed point onto the SDF zero set (Fit → Clamp until
    // |sdf| < targetEpsilon or fitMaxIterations) so it really finishes ON the
    // surface, within its cell — the goal of the relaxation. With repulsion
    // off this IS the pure Fit-on-Surface. Also decides "only on surface":
    // a point clamped short of the zero set never reaches epsilon and drops.
    if opts.fit_on_surface {
        for (i, leaf) in leaves.iter().enumerate() {
            if !process[i] {
                continue;
            }
            for _ in 0..opts.fit_max_iterations {
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
            fit_max_iterations: 20,
            fit_target_epsilon: 1e-4,
            fit_step_multiplier: 0.8,
            constrain_to_cell: false,
            ..Default::default()
        };
        let (pts, process, _) = optimize_leaf_points(&leaves, &opts, &sphere_sdf, &sphere_grad);
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
            fit_max_iterations: 50,
            fit_target_epsilon: 1e-4,
            fit_step_multiplier: 1.0,
            constrain_to_cell: true,
            ..Default::default()
        };
        let (pts, _, _) = optimize_leaf_points(&[leaf], &opts, &sphere_sdf, &sphere_grad);
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
            fit_max_iterations: 20,
            fit_target_epsilon: 1e-4,
            constrain_to_cell: false,
            ..Default::default()
        };
        let (pts, process, _) =
            optimize_leaf_points(&[near, deep], &opts, &sphere_sdf, &sphere_grad);
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
            fit_max_iterations: 20,
            fit_target_epsilon: 1e-4,
            constrain_to_cell: false,
            ..Default::default()
        };
        let (pts, process, _) =
            optimize_leaf_points(&[straddling, inside], &opts, &sphere_sdf, &sphere_grad);
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
            repulsion_iterations: 5,
            interaction_radius: 3.0,
            repulsion_strength: 0.5,
            fit_step_multiplier: 0.8,
            fit_target_epsilon: 1e-4,
            constrain_to_cell: false,
            ..Default::default()
        };

        // Fit OFF → no fit step, lone point, no neighbours → unchanged.
        let no_fit = MesherPointOptions { fit_on_surface: false, ..base.clone() };
        let (p_nofit, _, _) = optimize_leaf_points(&[leaf], &no_fit, &sphere_sdf, &sphere_grad);
        assert_eq!(p_nofit[0], start, "no fit + no neighbours must not move");

        // Fit ON → the phase-2 fit step pulls it onto the surface.
        let with_fit = MesherPointOptions { fit_on_surface: true, ..base };
        let (p_fit, _, _) = optimize_leaf_points(&[leaf], &with_fit, &sphere_sdf, &sphere_grad);
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
            repulsion_iterations: 8,
            interaction_radius: 3.0,
            repulsion_strength: 0.2,
            weighting_function: WeightingFunction::Linear,
            fit_max_iterations: 8,
            fit_target_epsilon: 1e-4,
            constrain_to_cell: false,
            ..Default::default()
        };
        let (pts, _, _) = optimize_leaf_points(&leaves, &opts, &sphere_sdf, &sphere_grad);
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
    fn inverse_square_weighting_also_spreads_and_stays_on_surface() {
        let leaves = vec![
            leaf_at([10.0, 0.0, 0.0], 8.0),
            leaf_at([9.995, 0.3, 0.0], 8.0),
        ];
        let opts = MesherPointOptions {
            repulsion: true,
            repulsion_iterations: 4,
            interaction_radius: 2.0,
            repulsion_strength: 0.1,
            weighting_function: WeightingFunction::InverseSquare,
            // Repulsion's per-step Fit keeps the points on the surface
            // (ticket yanv: Repulsion → Fit → Clamp).
            fit_on_surface: true,
            fit_target_epsilon: 1e-4,
            constrain_to_cell: false,
            ..Default::default()
        };
        let (pts, _, _) = optimize_leaf_points(&leaves, &opts, &sphere_sdf, &sphere_grad);
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
            fit_max_iterations: 30,
            fit_target_epsilon: 1e-4,
            fit_step_multiplier: 1.0,
            constrain_to_cell: true,
            ..Default::default()
        };
        let (pts, _, alive) =
            optimize_leaf_points(&[reaching, deep], &opts, &sphere_sdf, &sphere_grad);
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
