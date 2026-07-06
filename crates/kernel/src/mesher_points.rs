//! Debug ▸ Mesher — iterative point optimisation on the narrow-band octree
//! (ticket jq2t).
//!
//! The octree scaffold (`api::export::octree_leaves`) yields exactly one
//! point per leaf cell (its centre). This module runs two optional, iterative
//! optimisers over a FILTERED subset of those points, entirely kernel-side
//! (a JS round-trip per SDF eval would be hopeless at 200k points):
//!
//!  1. **Fit on Surface** — damped Newton projection onto the SDF zero set:
//!     `p ← p − stepMultiplier · d · ∇f/|∇f|` until `|d| < targetEpsilon` or
//!     `maxIterations`.
//!  2. **Repulsion inside Surface** — tangential relaxation: per iteration a
//!     repulsion force from all neighbours within `interactionRadius`
//!     (uniform-grid hash), projected into the tangent plane, applied, then
//!     re-pinned to the surface with a short Newton fit (≤3 steps).
//!
//! Cross-cutting constraints:
//!  - `constrainToCell`: after EVERY position update the point is clamped to
//!    its origin leaf cell's bounds (topological protection — one point per
//!    cell stays one point per cell).
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
    pub fit_max_iterations: u32,
    pub fit_target_epsilon: f32,
    pub fit_step_multiplier: f32,
    // Feature 2: tangential relaxation.
    pub repulsion: bool,
    pub repulsion_iterations: u32,
    pub interaction_radius: f32,
    pub repulsion_strength: f32,
    pub weighting_function: WeightingFunction,
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
            fit_max_iterations: 8,
            fit_target_epsilon: 0.001,
            fit_step_multiplier: 0.8,
            repulsion: false,
            repulsion_iterations: 4,
            interaction_radius: 2.0,
            repulsion_strength: 0.5,
            weighting_function: WeightingFunction::Linear,
        }
    }
}

fn clamp_to_cell(p: &mut [f32; 3], leaf: &Leaf) {
    let h = leaf.size * 0.5;
    for a in 0..3 {
        p[a] = p[a].clamp(leaf.center[a] - h, leaf.center[a] + h);
    }
}

/// Damped Newton projection of `p` onto the zero set (feature 1; also the
/// re-pin step of feature 2). `grad_eps` is the central-difference step.
#[allow(clippy::too_many_arguments)]
fn newton_fit(
    p: &mut [f32; 3],
    leaf: &Leaf,
    sdf: &dyn Fn([f32; 3]) -> f32,
    grad: &dyn Fn([f32; 3]) -> [f32; 3],
    max_iterations: u32,
    target_epsilon: f32,
    step_multiplier: f32,
    constrain_to_cell: bool,
) {
    for _ in 0..max_iterations {
        let d = sdf(*p);
        if d.abs() < target_epsilon {
            break;
        }
        let n = grad(*p);
        let step = step_multiplier * d;
        p[0] -= step * n[0];
        p[1] -= step * n[1];
        p[2] -= step * n[2];
        if constrain_to_cell {
            clamp_to_cell(p, leaf);
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
) -> (Vec<[f32; 3]>, Vec<bool>) {
    let mut points: Vec<[f32; 3]> = leaves.iter().map(|l| l.center).collect();

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

    // ── Feature 1: Fit on Surface ──
    if opts.fit_on_surface {
        let step = opts.fit_step_multiplier.clamp(0.1, 1.0);
        for (i, leaf) in leaves.iter().enumerate() {
            if !process[i] {
                continue;
            }
            newton_fit(
                &mut points[i],
                leaf,
                sdf,
                grad,
                opts.fit_max_iterations,
                opts.fit_target_epsilon.max(0.0),
                step,
                opts.constrain_to_cell,
            );
        }
    }

    // ── Feature 2: Repulsion inside Surface (tangential relaxation) ──
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
        // ignored by the algorithms entirely, per the ticket.
        let active: Vec<usize> = (0..leaves.len()).filter(|&i| process[i]).collect();
        // The re-pin after each tangential move (ticket: "2-3 iterations").
        let repin_iters = 3u32.min(opts.fit_max_iterations.max(1));
        for _ in 0..opts.repulsion_iterations {
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
                if f == [0.0, 0.0, 0.0] {
                    continue;
                }
                // Project the force into the tangent plane of the surface.
                let n = grad(p);
                let fn_dot = f[0] * n[0] + f[1] * n[1] + f[2] * n[2];
                let ft = [f[0] - fn_dot * n[0], f[1] - fn_dot * n[1], f[2] - fn_dot * n[2]];
                let leaf = &leaves[i];
                let mut np = [
                    p[0] + ft[0] * opts.repulsion_strength,
                    p[1] + ft[1] * opts.repulsion_strength,
                    p[2] + ft[2] * opts.repulsion_strength,
                ];
                if opts.constrain_to_cell {
                    clamp_to_cell(&mut np, leaf);
                }
                // Re-pin to the surface — the tangential move drifts off the
                // exact zero set on curved geometry.
                newton_fit(
                    &mut np,
                    leaf,
                    sdf,
                    grad,
                    repin_iters,
                    opts.fit_target_epsilon.max(0.0),
                    opts.fit_step_multiplier.clamp(0.1, 1.0),
                    opts.constrain_to_cell,
                );
                moved[i] = np;
            }
            points = moved;
        }
    }

    (points, process)
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
        let (pts, process) = optimize_leaf_points(&leaves, &opts, &sphere_sdf, &sphere_grad);
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
        let (pts, _) = optimize_leaf_points(&[leaf], &opts, &sphere_sdf, &sphere_grad);
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
        let (pts, process) =
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
        let (pts, process) =
            optimize_leaf_points(&[straddling, inside], &opts, &sphere_sdf, &sphere_grad);
        assert!(process[0], "straddling cell must be processed");
        assert!(!process[1], "interior cell must be rigid");
        assert!(sphere_sdf(pts[0]).abs() < 1e-3);
        assert_eq!(pts[1], inside.center);
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
        let (pts, _) = optimize_leaf_points(&leaves, &opts, &sphere_sdf, &sphere_grad);
        let after = min_dist(&pts);
        assert!(
            after > before * 2.0,
            "repulsion did not spread the cluster: {before} → {after}"
        );
        for p in &pts {
            assert!(
                sphere_sdf(*p).abs() < 1e-2,
                "repulsion left the surface: {p:?} (d={})",
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
            fit_target_epsilon: 1e-4,
            constrain_to_cell: false,
            ..Default::default()
        };
        let (pts, _) = optimize_leaf_points(&leaves, &opts, &sphere_sdf, &sphere_grad);
        let d = [
            pts[0][0] - pts[1][0],
            pts[0][1] - pts[1][1],
            pts[0][2] - pts[1][2],
        ];
        let dist = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        assert!(dist > 0.3, "points did not separate: {dist}");
        for p in &pts {
            assert!(sphere_sdf(*p).abs() < 1e-2);
        }
    }
}
