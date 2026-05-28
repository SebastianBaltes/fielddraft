//! Fine-tuning loop: re-project tangency points onto the evolving
//! sPSR mesh, then back onto sphere surfaces.

use super::closest_point::ClosestPointBvh;
use super::sphere_tree::{SdfSphere, SphereAabbTree};

/// One iteration of fine-tuning. Returns true if any tangency point moved.
pub fn finetune_iteration(
    spheres: &[SdfSphere],
    tree: &SphereAabbTree,
    tangency_positions: &mut [[f32; 3]],
    tangency_active: &[bool],
    mesh_verts: &[f32],
    mesh_tris: &[u32],
) -> bool {
    if mesh_verts.is_empty() || mesh_tris.is_empty() {
        return false;
    }

    let bvh = ClosestPointBvh::build(mesh_verts, mesh_tris);
    let mut any_moved = false;
    let tau = 0.01f32;

    for (si, sphere) in spheres.iter().enumerate() {
        if !tangency_active[si] {
            continue;
        }

        let (closest, dist) = bvh.closest_point(sphere.center);
        if dist < 1e-10 {
            continue;
        }

        // Project closest mesh point onto sphere surface
        let dx = closest[0] - sphere.center[0];
        let dy = closest[1] - sphere.center[1];
        let dz = closest[2] - sphere.center[2];
        let len = (dx * dx + dy * dy + dz * dz).sqrt();
        if len < 1e-10 {
            continue;
        }

        let sign = if sphere.positive { 1.0f32 } else { -1.0 };
        let signed_dist = if dist < sphere.radius { -sign } else { sign };
        let _ = signed_dist;

        let q = [
            sphere.center[0] + sphere.radius * dx / len,
            sphere.center[1] + sphere.radius * dy / len,
            sphere.center[2] + sphere.radius * dz / len,
        ];

        // Move tangency point towards q along geodesic on sphere
        let old = tangency_positions[si];
        let new_pos = geodesic_move(old, q, sphere, tau);

        // Feasibility check
        if !tree.point_inside_any_same_sign(spheres, new_pos, sphere.positive, si) {
            let ddx = new_pos[0] - old[0];
            let ddy = new_pos[1] - old[1];
            let ddz = new_pos[2] - old[2];
            if ddx * ddx + ddy * ddy + ddz * ddz > 1e-12 {
                any_moved = true;
            }
            tangency_positions[si] = new_pos;
        }
    }

    any_moved
}

fn geodesic_move(from: [f32; 3], to: [f32; 3], sphere: &SdfSphere, tau: f32) -> [f32; 3] {
    let d_from = [
        from[0] - sphere.center[0],
        from[1] - sphere.center[1],
        from[2] - sphere.center[2],
    ];
    let d_to = [
        to[0] - sphere.center[0],
        to[1] - sphere.center[1],
        to[2] - sphere.center[2],
    ];

    let len_from = (d_from[0] * d_from[0] + d_from[1] * d_from[1] + d_from[2] * d_from[2]).sqrt();
    let len_to = (d_to[0] * d_to[0] + d_to[1] * d_to[1] + d_to[2] * d_to[2]).sqrt();

    if len_from < 1e-10 || len_to < 1e-10 {
        return to;
    }

    let n_from = [d_from[0] / len_from, d_from[1] / len_from, d_from[2] / len_from];
    let n_to = [d_to[0] / len_to, d_to[1] / len_to, d_to[2] / len_to];

    let dot = (n_from[0] * n_to[0] + n_from[1] * n_to[1] + n_from[2] * n_to[2]).clamp(-1.0, 1.0);
    let angle = dot.acos();

    if angle < tau {
        return to;
    }

    // Slerp
    let t = 1.0 - tau / angle;
    let sin_angle = angle.sin();
    if sin_angle.abs() < 1e-10 {
        return to;
    }

    let w_from = ((1.0 - t) * angle).sin() / sin_angle;
    let w_to = (t * angle).sin() / sin_angle;

    let result_dir = [
        w_from * n_from[0] + w_to * n_to[0],
        w_from * n_from[1] + w_to * n_to[1],
        w_from * n_from[2] + w_to * n_to[2],
    ];

    let len_r = (result_dir[0] * result_dir[0]
        + result_dir[1] * result_dir[1]
        + result_dir[2] * result_dir[2])
    .sqrt();
    if len_r < 1e-10 {
        return to;
    }

    [
        sphere.center[0] + sphere.radius * result_dir[0] / len_r,
        sphere.center[1] + sphere.radius * result_dir[1] / len_r,
        sphere.center[2] + sphere.radius * result_dir[2] / len_r,
    ]
}
