//! sPSR wrapper around the `poisson_reconstruction` crate.

use nalgebra::{Point3, Vector3};
use poisson_reconstruction::PoissonReconstruction;

/// Run screened Poisson surface reconstruction on oriented point cloud.
/// Returns (flat f32 positions, flat u32 triangle indices) or None on failure.
pub fn reconstruct(
    points: &[[f32; 3]],
    normals: &[[f32; 3]],
) -> Option<(Vec<f32>, Vec<u32>)> {
    if points.is_empty() || points.len() != normals.len() {
        return None;
    }

    let pts: Vec<Point3<f64>> = points
        .iter()
        .map(|p| Point3::new(p[0] as f64, p[1] as f64, p[2] as f64))
        .collect();
    let nrm: Vec<Vector3<f64>> = normals
        .iter()
        .map(|n| Vector3::new(n[0] as f64, n[1] as f64, n[2] as f64))
        .collect();

    let depth = adaptive_depth(points.len());

    let psr = PoissonReconstruction::from_points_and_normals(
        &pts,
        &nrm,
        10.0,
        depth.saturating_sub(2).max(2),
        depth,
        3,
    );

    let buffers = psr.reconstruct_mesh_buffers();
    let verts = buffers.vertices();
    let idxs = buffers.indices();

    if verts.is_empty() || idxs.is_empty() {
        return None;
    }

    let mut positions = Vec::with_capacity(verts.len() * 3);
    for v in verts {
        positions.push(v.x as f32);
        positions.push(v.y as f32);
        positions.push(v.z as f32);
    }

    let indices: Vec<u32> = idxs.to_vec();

    if indices.len() % 3 != 0 {
        return None;
    }

    Some((positions, indices))
}

fn adaptive_depth(n: usize) -> usize {
    if n < 1000 {
        4
    } else if n < 10000 {
        5
    } else if n < 100000 {
        6
    } else {
        7
    }
}
