//! Fine-tuning loop: iteratively improve tangent points by
//! re-projecting onto the evolving sPSR mesh via great-arc rotation
//! with line search, plus preventing-intersections logic.
//!
//! Ref: Paper Section 4.2 (Figure 9, 13), `fine_tune_point_cloud_iter.cpp`

use super::closest_point::ClosestPointBvh;
use super::feasibility::{make_feasible_or_resolve, tangent_normal, TangentPoint};
use super::sphere_tree::{SdfSphere, SphereAabbTree};

const TAU: f32 = 0.01;
const LINE_SEARCH_ITERS: u32 = 20;
const MAX_POINTS_PER_SPHERE: usize = 3;

/// One iteration of fine-tuning. Modifies `tangent_points` in place.
/// Returns true if any tangent point moved or new points were added.
pub fn finetune_iteration(
    spheres: &[SdfSphere],
    tree: &SphereAabbTree,
    tangent_points: &mut Vec<TangentPoint>,
    mesh_verts: &[f32],
    mesh_tris: &[u32],
    positive: bool,
) -> bool {
    if mesh_verts.is_empty() || mesh_tris.is_empty() {
        return false;
    }

    let bvh = ClosestPointBvh::build(mesh_verts, mesh_tris);
    let mut any_changed = false;

    // Collect indices of tangent points per sphere
    let mut sphere_tp_indices: Vec<Vec<usize>> = vec![vec![]; spheres.len()];
    for (ti, tp) in tangent_points.iter().enumerate() {
        if tp.sphere_idx < spheres.len() {
            sphere_tp_indices[tp.sphere_idx].push(ti);
        }
    }

    // Points to add/remove after the loop
    let mut points_to_add: Vec<(usize, [f32; 3])> = Vec::new();
    let mut indices_to_remove: Vec<usize> = Vec::new();

    for si in 0..spheres.len() {
        let sphere = &spheres[si];
        let tp_indices = &sphere_tp_indices[si];

        // Find closest point on mesh to sphere center
        let (closest_on_mesh, dist_to_mesh) = bvh.closest_point(sphere.center);

        if dist_to_mesh < 1e-10 {
            continue;
        }

        // Compute ideal tangent point q_i
        let mut dir = [
            closest_on_mesh[0] - sphere.center[0],
            closest_on_mesh[1] - sphere.center[1],
            closest_on_mesh[2] - sphere.center[2],
        ];
        let dir_len = (dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2]).sqrt();
        if dir_len < 1e-10 {
            continue;
        }
        dir = [dir[0] / dir_len, dir[1] / dir_len, dir[2] / dir_len];

        let q_i = [
            sphere.center[0] + sphere.radius * dir[0],
            sphere.center[1] + sphere.radius * dir[1],
            sphere.center[2] + sphere.radius * dir[2],
        ];

        if tp_indices.is_empty() {
            // "Second chance": sphere had no tangent point from initial phase.
            // Try q_i directly.
            if let Some(feasible) =
                make_feasible_or_resolve(q_i, si, spheres, tree, positive)
            {
                points_to_add.push((si, feasible));
                any_changed = true;
            }
            continue;
        }

        // Move each tangent point on this sphere toward q_i
        for &ti in tp_indices {
            let old_pos = tangent_points[ti].position;
            let new_pos = arc_move_with_line_search(
                old_pos, q_i, sphere, si, spheres, tree, positive,
            );

            let ddx = new_pos[0] - old_pos[0];
            let ddy = new_pos[1] - old_pos[1];
            let ddz = new_pos[2] - old_pos[2];
            if ddx * ddx + ddy * ddy + ddz * ddz > 1e-12 {
                any_changed = true;
                tangent_points[ti].position = new_pos;
                tangent_points[ti].normal = tangent_normal(new_pos, sphere);
            }
        }

        // Preventing intersections: if mesh intersects the sphere
        // (dist_to_mesh < radius), add extra tangent points
        if dist_to_mesh < sphere.radius {
            let current_count = tp_indices.len() + points_to_add.iter()
                .filter(|(s, _)| *s == si)
                .count();

            if current_count < MAX_POINTS_PER_SPHERE {
                let severe = dist_to_mesh < 0.25 * sphere.radius;

                if !severe && tp_indices.len() >= MAX_POINTS_PER_SPHERE {
                    // Light case: discard the point farthest from mesh,
                    // then add new at q_i
                    let farthest_ti = tp_indices
                        .iter()
                        .copied()
                        .max_by(|&a, &b| {
                            let da = bvh.closest_point(tangent_points[a].position).1;
                            let db = bvh.closest_point(tangent_points[b].position).1;
                            da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
                        });
                    if let Some(ti) = farthest_ti {
                        indices_to_remove.push(ti);
                    }
                }

                if let Some(feasible) =
                    make_feasible_or_resolve(q_i, si, spheres, tree, positive)
                {
                    points_to_add.push((si, feasible));
                    any_changed = true;
                }
            }
        }
    }

    // Remove discarded points (reverse sort to keep indices valid)
    indices_to_remove.sort_unstable();
    indices_to_remove.dedup();
    for &ti in indices_to_remove.iter().rev() {
        if ti < tangent_points.len() {
            tangent_points.swap_remove(ti);
        }
    }

    // Add new tangent points
    for (si, pos) in points_to_add {
        let sphere = &spheres[si];
        tangent_points.push(TangentPoint {
            position: pos,
            normal: tangent_normal(pos, sphere),
            sphere_idx: si,
        });
    }

    // Re-check feasibility of all points
    for tp in tangent_points.iter_mut() {
        let si = tp.sphere_idx;
        if si >= spheres.len() {
            continue;
        }
        if tree.point_inside_any_same_sign(spheres, tp.position, positive, si) {
            // Try to resolve
            if let Some(resolved) =
                make_feasible_or_resolve(tp.position, si, spheres, tree, positive)
            {
                tp.position = resolved;
                tp.normal = tangent_normal(resolved, &spheres[si]);
                any_changed = true;
            }
            // If resolution fails, we keep the point as-is (sPSR is robust to some noise)
        }
    }

    any_changed
}

/// Move `from` toward `to` on the great circle of `sphere`, with
/// line search: if the rotated point is infeasible, halve the angle.
fn arc_move_with_line_search(
    from: [f32; 3],
    to: [f32; 3],
    sphere: &SdfSphere,
    sphere_idx: usize,
    spheres: &[SdfSphere],
    tree: &SphereAabbTree,
    positive: bool,
) -> [f32; 3] {
    let c = sphere.center;
    let r = sphere.radius;

    let d_from = [from[0] - c[0], from[1] - c[1], from[2] - c[2]];
    let d_to = [to[0] - c[0], to[1] - c[1], to[2] - c[2]];

    let len_from = (d_from[0] * d_from[0] + d_from[1] * d_from[1] + d_from[2] * d_from[2]).sqrt();
    let len_to = (d_to[0] * d_to[0] + d_to[1] * d_to[1] + d_to[2] * d_to[2]).sqrt();

    if len_from < 1e-10 || len_to < 1e-10 {
        return from;
    }

    let n_from = [d_from[0] / len_from, d_from[1] / len_from, d_from[2] / len_from];
    let n_to = [d_to[0] / len_to, d_to[1] / len_to, d_to[2] / len_to];

    // Compute the full angle between from and to on the sphere
    let dot = (n_from[0] * n_to[0] + n_from[1] * n_to[1] + n_from[2] * n_to[2]).clamp(-1.0, 1.0);

    // Half-angle formula: 2*arcsin(0.5*||from-to||/r) for numerical stability
    let half_chord = 0.5 * dist_3(from, to) / r;
    let full_angle = 2.0 * half_chord.min(1.0).asin();

    // Rotation angle: min(tau/r, full_angle)
    let mut theta = (TAU / r).min(full_angle);

    if theta < 1e-8 || full_angle < 1e-8 {
        return from;
    }

    // If full angle is tiny, just go to target
    if full_angle < 1e-6 {
        return to;
    }

    // Compute rotation axis: (d_from) x (d_to)
    let axis = cross3(d_from, d_to);
    let axis_len = (axis[0] * axis[0] + axis[1] * axis[1] + axis[2] * axis[2]).sqrt();
    if axis_len < 1e-10 {
        // from and to are parallel — either same point or antipodal
        if dot > 0.0 {
            return from; // Same direction, no move needed
        }
        // Antipodal: pick arbitrary perpendicular
        return from;
    }
    let axis_n = [axis[0] / axis_len, axis[1] / axis_len, axis[2] / axis_len];

    // Line search: try rotation, halve angle if infeasible
    for _ in 0..LINE_SEARCH_ITERS {
        let rotated = rotate_around_axis(d_from, axis_n, theta);
        let rot_len = (rotated[0] * rotated[0] + rotated[1] * rotated[1] + rotated[2] * rotated[2]).sqrt();
        let result = if rot_len > 1e-10 {
            [
                c[0] + r * rotated[0] / rot_len,
                c[1] + r * rotated[1] / rot_len,
                c[2] + r * rotated[2] / rot_len,
            ]
        } else {
            from
        };

        if !tree.point_inside_any_same_sign(spheres, result, positive, sphere_idx) {
            return result;
        }

        theta *= 0.5;
    }

    // All line search attempts failed — keep original position
    from
}

/// Rodrigues' rotation formula: rotate vector `v` around unit axis `k` by angle `theta`.
fn rotate_around_axis(v: [f32; 3], k: [f32; 3], theta: f32) -> [f32; 3] {
    let cos_t = theta.cos();
    let sin_t = theta.sin();
    let dot_kv = k[0] * v[0] + k[1] * v[1] + k[2] * v[2];
    let cross_kv = cross3(k, v);

    [
        v[0] * cos_t + cross_kv[0] * sin_t + k[0] * dot_kv * (1.0 - cos_t),
        v[1] * cos_t + cross_kv[1] * sin_t + k[1] * dot_kv * (1.0 - cos_t),
        v[2] * cos_t + cross_kv[2] * sin_t + k[2] * dot_kv * (1.0 - cos_t),
    ]
}

fn cross3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dist_3(a: [f32; 3], b: [f32; 3]) -> f32 {
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    let dz = a[2] - b[2];
    (dx * dx + dy * dy + dz * dz).sqrt()
}
