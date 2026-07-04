//! Mesh decimation via the `baby_shark` crate (edge-collapse QEM).
//!
//! The dense Surface Nets mesh (see [`crate::surface_nets`]) captures the
//! topology reliably but is far too heavy. We hand off the error-bounded
//! simplification to `baby_shark`'s `EdgeDecimator`: a battle-tested,
//! deterministic QEM edge-collapse that keeps the mesh manifold and
//! watertight. (This replaced a hand-written QEM whose `HashSet`-ordered
//! ring re-push was non-deterministic and occasionally left zero-area
//! slivers.) Pure Rust, wasm-safe.

use crate::dc::Mesh;
use baby_shark::decimation::{ConstantErrorDecimationCriteria, EdgeDecimator};
use baby_shark::exports::nalgebra::Vector3;
use baby_shark::mesh::corner_table::CornerTableF;
use baby_shark::mesh::traits::TriangleMesh;
use std::collections::HashMap;

/// Outcome of a decimation pass.
pub struct DecimateStats {
    pub collapses: usize,
    pub triangles_before: usize,
    pub triangles_after: usize,
}

/// Decimate `mesh` in place with error-bounded QEM edge collapses via
/// `baby_shark`. `max_error_sq` is the squared-distance-to-planes ceiling
/// (≈ the square of the allowed chordal deviation in mm). `baby_shark`'s
/// `ConstantErrorDecimationCriteria` compares against the quadric error
/// `q.error()`, which is the same summed-squared-distance metric — so we
/// pass `max_error_sq` straight through (no sqrt). The mesh stays manifold
/// and watertight; per-vertex normals are recomputed from the surviving
/// faces.
pub fn simplify_qem(mesh: &mut Mesh, max_error_sq: f32) -> DecimateStats {
    let vcount = mesh.positions.len() / 3;
    let tcount = mesh.indices.len() / 3;
    if vcount == 0 || tcount == 0 {
        return DecimateStats {
            collapses: 0,
            triangles_before: tcount,
            triangles_after: tcount,
        };
    }

    // Build a baby_shark CornerTable from our interleaved buffers.
    let verts: Vec<Vector3<f32>> = (0..vcount)
        .map(|i| {
            Vector3::new(
                mesh.positions[i * 3],
                mesh.positions[i * 3 + 1],
                mesh.positions[i * 3 + 2],
            )
        })
        .collect();
    let faces: Vec<usize> = mesh.indices.iter().map(|&i| i as usize).collect();

    let mut ct = CornerTableF::from_vertex_and_face_slices(&verts, &faces);

    let crit = ConstantErrorDecimationCriteria::new(max_error_sq);
    let mut decimator = EdgeDecimator::new().decimation_criteria(crit);
    decimator.decimate(&mut ct);

    // Read the decimated mesh back into compact interleaved buffers. The
    // inherent `faces()`/`vertices()` on CornerTable shadow the trait
    // methods and yield opaque `FaceId`/`VertexId`; call the `TriangleMesh`
    // trait methods explicitly to get vertex triples and positions.
    let mut remap: HashMap<_, u32> = HashMap::new();
    let mut positions: Vec<f32> = Vec::new();
    for vid in TriangleMesh::vertices(&ct) {
        let p = TriangleMesh::position(&ct, vid);
        remap.insert(vid, (positions.len() / 3) as u32);
        positions.push(p[0]);
        positions.push(p[1]);
        positions.push(p[2]);
    }
    let mut indices: Vec<u32> = Vec::new();
    for f in TriangleMesh::faces(&ct) {
        let (a, b, c) = (remap[&f[0]], remap[&f[1]], remap[&f[2]]);
        if a == b || b == c || a == c {
            continue;
        }
        indices.push(a);
        indices.push(b);
        indices.push(c);
    }

    // Area-weighted vertex normals from the survivors.
    let vc = positions.len() / 3;
    let mut normals = vec![0.0f32; positions.len()];
    for t in indices.chunks_exact(3) {
        let (i, j, k) = (t[0] as usize, t[1] as usize, t[2] as usize);
        let p = |a: usize| {
            [
                positions[a * 3],
                positions[a * 3 + 1],
                positions[a * 3 + 2],
            ]
        };
        let (a, b, c) = (p(i), p(j), p(k));
        let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let ac = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let n = [
            ab[1] * ac[2] - ab[2] * ac[1],
            ab[2] * ac[0] - ab[0] * ac[2],
            ab[0] * ac[1] - ab[1] * ac[0],
        ];
        for &idx in &[i, j, k] {
            normals[idx * 3] += n[0];
            normals[idx * 3 + 1] += n[1];
            normals[idx * 3 + 2] += n[2];
        }
    }
    for i in 0..vc {
        let n = [normals[i * 3], normals[i * 3 + 1], normals[i * 3 + 2]];
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        if len > 1e-12 {
            normals[i * 3] = n[0] / len;
            normals[i * 3 + 1] = n[1] / len;
            normals[i * 3 + 2] = n[2] / len;
        } else {
            normals[i * 3 + 1] = 1.0;
        }
    }

    let triangles_after = indices.len() / 3;
    let collapses = vcount.saturating_sub(vc);
    mesh.positions = positions;
    mesh.normals = normals;
    mesh.indices = indices;
    mesh.classifications = Vec::new();
    mesh.contributors = Vec::new();
    mesh.face_index = Vec::new();

    DecimateStats {
        collapses,
        triangles_before: tcount,
        triangles_after,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh_validate::validate_topology;

    /// A closed octahedron (6 verts, 8 triangles) — the minimal manifold
    /// stress test: whatever the decimator does, it must not open a
    /// boundary or create a non-manifold edge.
    fn octahedron() -> Mesh {
        let positions = vec![
            1.0, 0.0, 0.0, //
            -1.0, 0.0, 0.0, //
            0.0, 1.0, 0.0, //
            0.0, -1.0, 0.0, //
            0.0, 0.0, 1.0, //
            0.0, 0.0, -1.0, //
        ];
        // Outward-consistent winding.
        let indices = vec![
            0, 2, 4, 2, 1, 4, 1, 3, 4, 3, 0, 4, // top fan
            2, 0, 5, 1, 2, 5, 3, 1, 5, 0, 3, 5, // bottom fan
        ];
        Mesh {
            positions,
            normals: vec![0.0; 18],
            indices,
            ..Mesh::default()
        }
    }

    #[test]
    fn decimation_preserves_watertightness() {
        let mut mesh = octahedron();
        assert!(validate_topology(&mesh).watertight);
        // A generous error budget invites aggressive collapses.
        simplify_qem(&mut mesh, 100.0);
        let v = validate_topology(&mesh);
        assert_eq!(v.boundary_edges, 0, "decimation opened a boundary");
        assert_eq!(v.non_manifold_edges, 0, "decimation went non-manifold");
        // Never leaves a degenerate soup: still a closed solid or empty.
        assert!(mesh.triangle_count() == 0 || v.watertight);
    }

    #[test]
    fn empty_mesh_is_noop() {
        let mut mesh = Mesh::default();
        let stats = simplify_qem(&mut mesh, 1.0);
        assert_eq!(stats.collapses, 0);
        assert_eq!(stats.triangles_after, 0);
    }
}
