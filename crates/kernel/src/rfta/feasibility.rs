//! Feasibility resolution: `resolve_collisions_on_sphere` and
//! `locally_make_feasible`.
//!
//! Ref: Paper Section 4.1, `resolve_collisions_on_sphere.cpp`,
//! `locally_make_feasible.cpp`

use super::sphere_tree::{SdfSphere, SphereAabbTree};

/// A tangent point on a sphere surface with its outward normal.
#[derive(Clone, Copy)]
pub struct TangentPoint {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub sphere_idx: usize,
}

/// For each sphere, compute an initial feasible tangent point.
///
/// `spheres` and `tree` must be the single-sign subset.
/// `outside_pts` are the free-space sample points from rasterization.
/// `indices` maps local sphere index → global sphere index.
///
/// Returns one `Option<TangentPoint>` per sphere: `Some` if a feasible
/// point was found, `None` if the sphere is fully occluded.
pub fn locally_make_feasible(
    spheres: &[SdfSphere],
    tree: &SphereAabbTree,
    outside_pts: &[[f32; 3]],
    positive: bool,
) -> Vec<Option<TangentPoint>> {
    let n = spheres.len();
    let k_search = (2.0 * (n as f64).cbrt()).ceil().max(4.0) as usize;
    let mut results = Vec::with_capacity(n);

    // Spatial grid over the outside points so the per-sphere nearest-point
    // lookup is ~O(1) instead of O(|outside_pts|). With one sphere per grid
    // corner the sphere count and outside-point count both scale with res³,
    // so the old linear search was O(spheres × outside_pts) — billions of
    // distance evals at res 32 — and dominated the whole pipeline.
    let point_grid = PointGrid::build(outside_pts);

    for si in 0..n {
        let sphere = &spheres[si];

        // 1. Find nearest outside point via the spatial grid
        let nearest = point_grid.nearest(sphere.center);
        let nearest_pt = match nearest {
            Some(p) => p,
            None => {
                results.push(None);
                continue;
            }
        };

        // 2. Project onto sphere surface
        let dx = nearest_pt[0] - sphere.center[0];
        let dy = nearest_pt[1] - sphere.center[1];
        let dz = nearest_pt[2] - sphere.center[2];
        let len = (dx * dx + dy * dy + dz * dz).sqrt();
        if len < 1e-10 {
            results.push(None);
            continue;
        }
        let mut candidate = [
            sphere.center[0] + sphere.radius * dx / len,
            sphere.center[1] + sphere.radius * dy / len,
            sphere.center[2] + sphere.radius * dz / len,
        ];

        // 3. Check feasibility
        if !tree.point_inside_any_same_sign(spheres, candidate, positive, si) {
            results.push(Some(make_tangent_point(candidate, sphere, si)));
            continue;
        }

        // 4. Resolve collisions iteratively using k closest spheres as search set
        let nearby = tree.k_closest_spheres(spheres, sphere.center, k_search, positive, si);
        let mut resolved = false;
        for _iter in 0..20 {
            // Collect blockers: spheres from the search set that contain the candidate
            let blockers: Vec<usize> = nearby
                .iter()
                .copied()
                .filter(|&bi| {
                    let b = &spheres[bi];
                    let bx = candidate[0] - b.center[0];
                    let by = candidate[1] - b.center[1];
                    let bz = candidate[2] - b.center[2];
                    bx * bx + by * by + bz * bz < b.radius * b.radius
                })
                .collect();
            // Also check via tree for any blockers outside the k-nearest set
            let tree_blockers = tree.get_spheres_containing(spheres, candidate, positive, si, 0.0);
            let mut all_blockers = blockers;
            for bi in tree_blockers {
                if !all_blockers.contains(&bi) {
                    all_blockers.push(bi);
                }
            }

            if all_blockers.is_empty() {
                resolved = true;
                break;
            }

            match resolve_collisions_on_sphere(candidate, si, spheres, &all_blockers) {
                Some(new_pt) => {
                    candidate = new_pt;
                    // Re-check feasibility with all spheres
                    if !tree.point_inside_any_same_sign(spheres, candidate, positive, si) {
                        resolved = true;
                        break;
                    }
                }
                None => break,
            }
        }

        if resolved {
            results.push(Some(make_tangent_point(candidate, sphere, si)));
        } else {
            results.push(None);
        }
    }

    results
}

/// Check if a point is feasible (not inside any other same-sign sphere)
/// and optionally resolve collisions. Returns the feasible point or None.
pub fn make_feasible_or_resolve(
    point: [f32; 3],
    sphere_idx: usize,
    spheres: &[SdfSphere],
    tree: &SphereAabbTree,
    positive: bool,
) -> Option<[f32; 3]> {
    if !tree.point_inside_any_same_sign(spheres, point, positive, sphere_idx) {
        return Some(point);
    }

    let mut candidate = point;
    for _iter in 0..20 {
        let blockers = tree.get_spheres_containing(spheres, candidate, positive, sphere_idx, 0.0);

        if blockers.is_empty() {
            return Some(candidate);
        }

        match resolve_collisions_on_sphere(candidate, sphere_idx, spheres, &blockers) {
            Some(new_pt) => {
                candidate = new_pt;
                if !tree.point_inside_any_same_sign(spheres, candidate, positive, sphere_idx) {
                    return Some(candidate);
                }
            }
            None => return None,
        }
    }

    None
}

fn make_tangent_point(pos: [f32; 3], sphere: &SdfSphere, idx: usize) -> TangentPoint {
    TangentPoint {
        position: pos,
        normal: tangent_normal(pos, sphere),
        sphere_idx: idx,
    }
}

/// Compute outward normal for a tangent point on a sphere.
///
/// For negative spheres (inside): normal = (point - center).normalized()
/// For positive spheres (outside): normal = -(point - center).normalized()
pub fn tangent_normal(pos: [f32; 3], sphere: &SdfSphere) -> [f32; 3] {
    let dx = pos[0] - sphere.center[0];
    let dy = pos[1] - sphere.center[1];
    let dz = pos[2] - sphere.center[2];
    let len = (dx * dx + dy * dy + dz * dz).sqrt();
    // sign(signed_radius) gives us -1 for negative (inside) and +1 for positive (outside)
    // We want: negative → normal outward (+1), positive → normal inward (-1)
    // So negate the sign of signed_radius.
    let sign = -sphere.signed_radius().signum();
    if len > 1e-10 {
        [sign * dx / len, sign * dy / len, sign * dz / len]
    } else {
        [0.0, sign, 0.0]
    }
}

/// Uniform spatial grid over a point set in (roughly) [0,1]³, supporting
/// approximate-free nearest-neighbour queries via an expanding shell search.
struct PointGrid {
    res: usize,
    inv_cell: f32,
    cell_size: f32,
    /// Flattened res³ buckets, each holding indices into `pts`.
    buckets: Vec<Vec<u32>>,
    pts: Vec<[f32; 3]>,
}

impl PointGrid {
    fn build(pts: &[[f32; 3]]) -> Self {
        let n = pts.len();
        // ~1 point per cell on average, clamped to a sane range.
        let res = ((n as f64).cbrt().ceil() as usize).clamp(1, 128);
        let mut buckets: Vec<Vec<u32>> = vec![Vec::new(); res * res * res];
        let inv_cell = res as f32; // points live in [0,1], so cell = 1/res
        let cell_size = 1.0 / res as f32;
        let clamp_idx = |v: f32| -> usize {
            let c = (v * inv_cell).floor();
            if c < 0.0 {
                0
            } else if c as usize >= res {
                res - 1
            } else {
                c as usize
            }
        };
        for (i, p) in pts.iter().enumerate() {
            let cx = clamp_idx(p[0]);
            let cy = clamp_idx(p[1]);
            let cz = clamp_idx(p[2]);
            buckets[cx + cy * res + cz * res * res].push(i as u32);
        }
        Self {
            res,
            inv_cell,
            cell_size,
            buckets,
            pts: pts.to_vec(),
        }
    }

    fn nearest(&self, q: [f32; 3]) -> Option<[f32; 3]> {
        if self.pts.is_empty() {
            return None;
        }
        let res = self.res as i32;
        let cell = |v: f32| -> i32 {
            let c = (v * self.inv_cell).floor() as i32;
            c.clamp(0, res - 1)
        };
        let (qx, qy, qz) = (cell(q[0]), cell(q[1]), cell(q[2]));

        let mut best_d2 = f32::INFINITY;
        let mut best: Option<[f32; 3]> = None;

        let mut radius = 0i32;
        loop {
            // Scan the Chebyshev shell at distance `radius` from the query cell.
            let lo = -radius;
            let hi = radius;
            for dz in lo..=hi {
                for dy in lo..=hi {
                    for dx in lo..=hi {
                        // Only the shell surface, not the filled cube.
                        if dx.abs() != radius && dy.abs() != radius && dz.abs() != radius {
                            continue;
                        }
                        let cx = qx + dx;
                        let cy = qy + dy;
                        let cz = qz + dz;
                        if cx < 0 || cx >= res || cy < 0 || cy >= res || cz < 0 || cz >= res {
                            continue;
                        }
                        let bucket =
                            &self.buckets[cx as usize + cy as usize * self.res + cz as usize * self.res * self.res];
                        for &pi in bucket {
                            let p = self.pts[pi as usize];
                            let ex = p[0] - q[0];
                            let ey = p[1] - q[1];
                            let ez = p[2] - q[2];
                            let d2 = ex * ex + ey * ey + ez * ez;
                            if d2 < best_d2 {
                                best_d2 = d2;
                                best = Some(p);
                            }
                        }
                    }
                }
            }

            // Once we have a candidate, any point in a shell at Chebyshev
            // distance `radius+1` is at least `radius * cell_size` away, so we
            // can stop as soon as that lower bound exceeds the best distance.
            if best.is_some() {
                let shell_min = radius as f32 * self.cell_size;
                if shell_min * shell_min > best_d2 {
                    break;
                }
            }
            radius += 1;
            if radius > res {
                break;
            }
        }

        best
    }
}

/// Core collision resolution: given a point `p` on sphere `sphere_idx`,
/// resolve it away from all `blockers`.
///
/// Ref: `resolve_collisions_on_sphere.cpp`
fn resolve_collisions_on_sphere(
    p: [f32; 3],
    sphere_idx: usize,
    spheres: &[SdfSphere],
    blockers: &[usize],
) -> Option<[f32; 3]> {
    if blockers.is_empty() {
        return Some(p);
    }

    let home = &spheres[sphere_idx];
    let c = home.center;
    let r = home.radius;

    if blockers.len() == 1 {
        // Single blocker: sphere-sphere intersection circle
        return resolve_one_blocker(p, c, r, &spheres[blockers[0]]);
    }

    // 2+ blockers: trilateration
    resolve_multiple_blockers(p, c, r, spheres, blockers)
}

/// Resolve collision with a single blocking sphere.
///
/// Computes the intersection circle of the home sphere (c, r) and
/// blocker sphere (d, s), then projects p onto that circle.
fn resolve_one_blocker(
    p: [f32; 3],
    c: [f32; 3],
    r: f32,
    blocker: &SdfSphere,
) -> Option<[f32; 3]> {
    let d = blocker.center;
    let s = blocker.radius;

    let cd = [d[0] - c[0], d[1] - c[1], d[2] - c[2]];
    let dist_sq = cd[0] * cd[0] + cd[1] * cd[1] + cd[2] * cd[2];
    let dist = dist_sq.sqrt();

    if dist < 1e-10 {
        return None;
    }

    // a = (r^2 - s^2 + dist^2) / (2*dist)
    let a = (r * r - s * s + dist_sq) / (2.0 * dist);
    let h_sq = r * r - a * a;
    if h_sq < 0.0 {
        return None; // No intersection
    }
    let h = h_sq.sqrt();

    // Circle center
    let axis = [cd[0] / dist, cd[1] / dist, cd[2] / dist];
    let mid = [
        c[0] + a * axis[0],
        c[1] + a * axis[1],
        c[2] + a * axis[2],
    ];

    // Project p onto the circle plane, then radially onto the circle
    let pm = [p[0] - mid[0], p[1] - mid[1], p[2] - mid[2]];
    let dot_axis = pm[0] * axis[0] + pm[1] * axis[1] + pm[2] * axis[2];
    let proj = [
        pm[0] - dot_axis * axis[0],
        pm[1] - dot_axis * axis[1],
        pm[2] - dot_axis * axis[2],
    ];
    let proj_len = (proj[0] * proj[0] + proj[1] * proj[1] + proj[2] * proj[2]).sqrt();

    if proj_len < 1e-10 {
        // p is on the axis — pick an arbitrary direction in the circle plane
        let perp = arbitrary_perpendicular(axis);
        return Some([
            mid[0] + h * perp[0],
            mid[1] + h * perp[1],
            mid[2] + h * perp[2],
        ]);
    }

    Some([
        mid[0] + h * proj[0] / proj_len,
        mid[1] + h * proj[1] / proj_len,
        mid[2] + h * proj[2] / proj_len,
    ])
}

/// Resolve collision with multiple blocking spheres via trilateration.
///
/// For each pair of blockers, compute trilateration with the home sphere
/// to find candidate intersection points. Pick the best valid candidate.
fn resolve_multiple_blockers(
    p: [f32; 3],
    c: [f32; 3],
    r: f32,
    spheres: &[SdfSphere],
    blockers: &[usize],
) -> Option<[f32; 3]> {
    let mut best: Option<[f32; 3]> = None;
    let mut best_dist_sq = f32::INFINITY;

    for bi in 0..blockers.len() {
        for bj in (bi + 1)..blockers.len() {
            let si = &spheres[blockers[bi]];
            let sj = &spheres[blockers[bj]];

            // Check for collinear centers
            let cd_i = [
                si.center[0] - c[0],
                si.center[1] - c[1],
                si.center[2] - c[2],
            ];
            let cd_j = [
                sj.center[0] - c[0],
                sj.center[1] - c[1],
                sj.center[2] - c[2],
            ];

            let cross = cross3(cd_i, cd_j);
            let cross_len_sq =
                cross[0] * cross[0] + cross[1] * cross[1] + cross[2] * cross[2];

            if cross_len_sq < 1e-10 {
                // Collinear: fall back to single-sphere intersection
                if let Some(pt) = resolve_one_blocker(p, c, r, si) {
                    let d2 = dist_sq_3(pt, p);
                    if is_valid_candidate(pt, spheres, blockers) && d2 < best_dist_sq {
                        best_dist_sq = d2;
                        best = Some(pt);
                    }
                }
                continue;
            }

            // Build local frame
            let di_len = (cd_i[0] * cd_i[0] + cd_i[1] * cd_i[1] + cd_i[2] * cd_i[2]).sqrt();
            if di_len < 1e-10 {
                continue;
            }

            let i_hat = [cd_i[0] / di_len, cd_i[1] / di_len, cd_i[2] / di_len];

            // j_hat = (cd_j - dot(cd_j, i_hat)*i_hat).normalized()
            let dot_ji = cd_j[0] * i_hat[0] + cd_j[1] * i_hat[1] + cd_j[2] * i_hat[2];
            let j_raw = [
                cd_j[0] - dot_ji * i_hat[0],
                cd_j[1] - dot_ji * i_hat[1],
                cd_j[2] - dot_ji * i_hat[2],
            ];
            let j_len = (j_raw[0] * j_raw[0] + j_raw[1] * j_raw[1] + j_raw[2] * j_raw[2]).sqrt();
            if j_len < 1e-10 {
                continue;
            }
            let j_hat = [j_raw[0] / j_len, j_raw[1] / j_len, j_raw[2] / j_len];

            let k_hat = cross3(i_hat, j_hat);

            // Solve for X, Y, Z
            let ri_sq = si.radius * si.radius;
            let rj_sq = sj.radius * sj.radius;
            let di_sq = di_len * di_len;

            let dj_len_sq = cd_j[0] * cd_j[0] + cd_j[1] * cd_j[1] + cd_j[2] * cd_j[2];

            // X = (r^2 - ri^2 + di^2) / (2*di)
            let big_x = (r * r - ri_sq + di_sq) / (2.0 * di_len);

            // dot(cd_j, j_hat) = j_len (the component in j direction)
            let dj_dot_j = cd_j[0] * j_hat[0] + cd_j[1] * j_hat[1] + cd_j[2] * j_hat[2];
            if dj_dot_j.abs() < 1e-10 {
                continue;
            }

            // Y = (r^2 - rj^2 + dj^2 - 2*dot(cd_j, i_hat)*X) / (2*dot(cd_j, j_hat))
            let big_y = (r * r - rj_sq + dj_len_sq - 2.0 * dot_ji * big_x) / (2.0 * dj_dot_j);

            // Z^2 = r^2 - X^2 - Y^2
            let z_sq = r * r - big_x * big_x - big_y * big_y;
            if z_sq < -1e-6 {
                continue;
            }
            let z_val = z_sq.max(0.0).sqrt();

            // Two candidates: c + X*i + Y*j +/- Z*k
            for &sign in &[1.0f32, -1.0f32] {
                let candidate = [
                    c[0] + big_x * i_hat[0] + big_y * j_hat[0] + sign * z_val * k_hat[0],
                    c[1] + big_x * i_hat[1] + big_y * j_hat[1] + sign * z_val * k_hat[1],
                    c[2] + big_x * i_hat[2] + big_y * j_hat[2] + sign * z_val * k_hat[2],
                ];

                // Check bounds [0,1]^3
                if candidate[0] < 0.0
                    || candidate[0] > 1.0
                    || candidate[1] < 0.0
                    || candidate[1] > 1.0
                    || candidate[2] < 0.0
                    || candidate[2] > 1.0
                {
                    continue;
                }

                // Check not inside other blockers (besides the two used for trilateration)
                if is_valid_candidate(candidate, spheres, blockers) {
                    let d2 = dist_sq_3(candidate, p);
                    if d2 < best_dist_sq {
                        best_dist_sq = d2;
                        best = Some(candidate);
                    }
                }
            }
        }
    }

    // If no trilateration candidate worked, try single-blocker fallback
    // with each blocker individually
    if best.is_none() {
        for &bi in blockers {
            if let Some(pt) = resolve_one_blocker(p, c, r, &spheres[bi]) {
                if is_valid_candidate(pt, spheres, blockers) {
                    let d2 = dist_sq_3(pt, p);
                    if d2 < best_dist_sq {
                        best_dist_sq = d2;
                        best = Some(pt);
                    }
                }
            }
        }
    }

    best
}

/// Check that candidate is not inside any of the blocker spheres.
fn is_valid_candidate(candidate: [f32; 3], spheres: &[SdfSphere], blockers: &[usize]) -> bool {
    for &bi in blockers {
        let b = &spheres[bi];
        let dx = candidate[0] - b.center[0];
        let dy = candidate[1] - b.center[1];
        let dz = candidate[2] - b.center[2];
        // Allow a small tolerance so points exactly on the boundary pass
        if dx * dx + dy * dy + dz * dz < b.radius * b.radius - 1e-6 {
            return false;
        }
    }
    true
}

fn cross3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dist_sq_3(a: [f32; 3], b: [f32; 3]) -> f32 {
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    let dz = a[2] - b[2];
    dx * dx + dy * dy + dz * dz
}

/// Find an arbitrary unit vector perpendicular to `v`.
fn arbitrary_perpendicular(v: [f32; 3]) -> [f32; 3] {
    let other = if v[0].abs() < 0.9 {
        [1.0, 0.0, 0.0]
    } else {
        [0.0, 1.0, 0.0]
    };
    let c = cross3(v, other);
    let len = (c[0] * c[0] + c[1] * c[1] + c[2] * c[2]).sqrt();
    if len < 1e-10 {
        return [1.0, 0.0, 0.0];
    }
    [c[0] / len, c[1] / len, c[2] / len]
}
