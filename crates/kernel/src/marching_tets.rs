//! Marching Tetrahedra — the robust Marching-Cubes-family mesher.
//!
//! Each grid cell is split into 6 tetrahedra along the main diagonal
//! (Freudenthal / Kuhn subdivision, consistent across cells so shared
//! faces triangulate identically → crack-free). A tetrahedron has no
//! ambiguous cases, so the result is watertight and 2-manifold **by
//! construction** for ANY field — the property plain Marching Cubes
//! can't guarantee at its saddle configurations. Only signs + edge
//! interpolation are used (no gradients, no distance bound), so a
//! non-1-Lipschitz field is meshed just as safely as a smooth one; it
//! rounds sharp edges (no feature preservation) and is triangle-heavy,
//! so the export pass decimates it afterwards.

use crate::bvh::BoundedCsg;
use crate::csg::EvalMode;
use crate::dc::{Mesh, DC_MARGIN_CELLS};
use std::collections::HashMap;

/// Corner offset (dx, dy, dz) for each of the 8 cube corners,
/// index = dx | dy<<1 | dz<<2.
const CUBE_CORNERS: [[usize; 3]; 8] = [
    [0, 0, 0],
    [1, 0, 0],
    [0, 1, 0],
    [1, 1, 0],
    [0, 0, 1],
    [1, 0, 1],
    [0, 1, 1],
    [1, 1, 1],
];

/// The 6 tetrahedra of a cube, all sharing the main diagonal 0→7. A
/// fixed decomposition means adjacent cells cut their shared face the
/// same way — no cracks.
const TETS: [[usize; 4]; 6] = [
    [0, 7, 1, 3],
    [0, 7, 3, 2],
    [0, 7, 2, 6],
    [0, 7, 6, 4],
    [0, 7, 4, 5],
    [0, 7, 5, 1],
];

/// Mesh a body with Marching Tetrahedra at `resolution` cells along the
/// longest AABB axis. Empty scene → empty mesh.
pub fn marching_tets(node: &BoundedCsg, resolution: u32) -> Mesh {
    if resolution == 0 {
        return Mesh::default();
    }
    let raw = node.aabb();
    if raw.is_empty() {
        return Mesh::default();
    }
    let size = raw.size();
    let max_dim = size[0].max(size[1]).max(size[2]);
    if max_dim <= 0.0 {
        return Mesh::default();
    }
    let cell = max_dim / resolution as f32;
    let grid = raw.expanded(cell * DC_MARGIN_CELLS);
    let gmin = grid.min;
    let gsize = grid.size();

    let nx = (gsize[0] / cell).ceil().max(1.0) as usize;
    let ny = (gsize[1] / cell).ceil().max(1.0) as usize;
    let nz = (gsize[2] / cell).ceil().max(1.0) as usize;
    let (cx, cy) = (nx + 1, ny + 1);

    let corner_pos = |x: usize, y: usize, z: usize| -> [f32; 3] {
        [
            gmin[0] + x as f32 * cell,
            gmin[1] + y as f32 * cell,
            gmin[2] + z as f32 * cell,
        ]
    };
    let corner_idx = |x: usize, y: usize, z: usize| -> usize { (z * cy + y) * cx + x };

    // Sample the field at every corner.
    let mut values = vec![0.0f32; cx * cy * (nz + 1)];
    for z in 0..=nz {
        for y in 0..=ny {
            for x in 0..=nx {
                values[corner_idx(x, y, z)] = node.sdf(corner_pos(x, y, z), EvalMode::Visual);
            }
        }
    }

    // March every tet of every cell into a triangle soup.
    let mut soup: Vec<[f32; 3]> = Vec::new();
    for z in 0..nz {
        for y in 0..ny {
            for x in 0..nx {
                let mut cpos = [[0.0f32; 3]; 8];
                let mut cval = [0.0f32; 8];
                let mut has_neg = false;
                let mut has_pos = false;
                for (c, off) in CUBE_CORNERS.iter().enumerate() {
                    let (px, py, pz) = (x + off[0], y + off[1], z + off[2]);
                    cpos[c] = corner_pos(px, py, pz);
                    let v = values[corner_idx(px, py, pz)];
                    cval[c] = v;
                    has_neg |= v < 0.0;
                    has_pos |= v >= 0.0;
                }
                if !(has_neg && has_pos) {
                    continue; // cell entirely inside or outside
                }
                for tet in &TETS {
                    march_tet(
                        [cpos[tet[0]], cpos[tet[1]], cpos[tet[2]], cpos[tet[3]]],
                        [cval[tet[0]], cval[tet[1]], cval[tet[2]], cval[tet[3]]],
                        &mut soup,
                    );
                }
            }
        }
    }
    if soup.is_empty() {
        return Mesh::default();
    }

    // Weld coincident crossings (edge crossings shared by adjacent tets /
    // cells are computed identically, so they weld exactly) into an
    // indexed mesh; drop any degenerate triangle.
    let (positions, indices) = weld(&soup);
    if indices.is_empty() {
        return Mesh::default();
    }

    let vcount = positions.len() / 3;
    let mut normals = vec![0.0f32; positions.len()];
    for i in 0..vcount {
        let p = [positions[i * 3], positions[i * 3 + 1], positions[i * 3 + 2]];
        let n = crate::dc::sdf_normal(node, p, cell * 0.25);
        normals[i * 3] = n[0];
        normals[i * 3 + 1] = n[1];
        normals[i * 3 + 2] = n[2];
    }

    Mesh {
        positions,
        normals,
        indices,
        ..Mesh::default()
    }
}

/// Emit the (unambiguous) surface triangles of one tetrahedron into the
/// soup. Winding is left to the export pass's `fix_orientation`.
fn march_tet(corners: [[f32; 3]; 4], vals: [f32; 4], soup: &mut Vec<[f32; 3]>) {
    let inside = [vals[0] < 0.0, vals[1] < 0.0, vals[2] < 0.0, vals[3] < 0.0];
    let inl: Vec<usize> = (0..4).filter(|&i| inside[i]).collect();
    let outl: Vec<usize> = (0..4).filter(|&i| !inside[i]).collect();
    let edge = |i: usize, j: usize| -> [f32; 3] {
        let (a, b) = (vals[i], vals[j]);
        let denom = a - b;
        let t = if denom.abs() < 1e-20 { 0.5 } else { a / denom };
        let (pa, pb) = (corners[i], corners[j]);
        [
            pa[0] + t * (pb[0] - pa[0]),
            pa[1] + t * (pb[1] - pa[1]),
            pa[2] + t * (pb[2] - pa[2]),
        ]
    };
    match (inl.len(), outl.len()) {
        (1, 3) => {
            let s = inl[0];
            soup.push(edge(s, outl[0]));
            soup.push(edge(s, outl[1]));
            soup.push(edge(s, outl[2]));
        }
        (3, 1) => {
            let s = outl[0];
            soup.push(edge(s, inl[0]));
            soup.push(edge(s, inl[1]));
            soup.push(edge(s, inl[2]));
        }
        (2, 2) => {
            let (i0, i1) = (inl[0], inl[1]);
            let (o0, o1) = (outl[0], outl[1]);
            let a = edge(i0, o0);
            let b = edge(i1, o0);
            let c = edge(i1, o1);
            let d = edge(i0, o1);
            soup.push(a);
            soup.push(b);
            soup.push(c);
            soup.push(a);
            soup.push(c);
            soup.push(d);
        }
        _ => {}
    }
}

/// Weld a triangle soup into an indexed mesh by exact-bit position, then
/// drop triangles that collapsed to a repeated vertex.
fn weld(soup: &[[f32; 3]]) -> (Vec<f32>, Vec<u32>) {
    let mut canon: HashMap<[u32; 3], u32> = HashMap::new();
    let mut positions: Vec<f32> = Vec::new();
    let mut ids: Vec<u32> = Vec::with_capacity(soup.len());
    for p in soup {
        let key = [p[0].to_bits(), p[1].to_bits(), p[2].to_bits()];
        let id = *canon.entry(key).or_insert_with(|| {
            let id = (positions.len() / 3) as u32;
            positions.extend_from_slice(p);
            id
        });
        ids.push(id);
    }
    let mut indices: Vec<u32> = Vec::with_capacity(ids.len());
    for t in ids.chunks_exact(3) {
        if t[0] == t[1] || t[1] == t[2] || t[0] == t[2] {
            continue;
        }
        indices.extend_from_slice(t);
    }
    (positions, indices)
}
