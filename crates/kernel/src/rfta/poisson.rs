//! sPSR wrapper around the `poisson_reconstruction` crate.

use nalgebra::{Point3, Vector3};
use poisson_reconstruction::PoissonReconstruction;

/// Run screened Poisson surface reconstruction on oriented point cloud.
///
/// `depth` is the sPSR octree depth — it governs output detail and dominates
/// runtime (cost scales with the octree node count ≈ 8^depth near the
/// surface). It is derived from the *requested mesh resolution*, not the input
/// point count: a depth of d resolves a 2^d grid, so res 32 → depth 5 matches
/// the display grid exactly. Keying off point count instead (the old
/// `adaptive_depth`) pushed res-32's 30k tangent points to depth 6 (=64³),
/// which made the unoptimized solver take ~50 s and stalled the worker.
///
/// Returns (flat f32 positions, flat u32 triangle indices) or None on failure.
pub fn reconstruct(
    points: &[[f32; 3]],
    normals: &[[f32; 3]],
    depth: usize,
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

/// sPSR octree depth for a requested mesh resolution. A depth of d resolves a
/// 2^d grid, so we pick the smallest depth whose grid is at least as fine as
/// the requested resolution, clamped to a range the solver handles in
/// interactive time.
pub fn depth_for_resolution(resolution: u32) -> usize {
    let d = (resolution.max(1) as f32).log2().ceil() as usize;
    d.clamp(3, 6)
}
