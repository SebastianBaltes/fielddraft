//! Built-in dense Surface Nets — the from-scratch alternative to the
//! `fast-surface-nets`-backed [`crate::surface_nets`] path.
//!
//! Same idea (one dual vertex per sign-changing cell on a uniform grid,
//! a quad per interior sign-changing edge → watertight by construction),
//! implemented directly against our SDF so the export dialog can offer a
//! dependency-free alternative and diff it against the library mesher.
//! Vertex placement is the naive centroid of the cell's edge crossings,
//! then a few Newton steps onto the exact isosurface.

use crate::bvh::BoundedCsg;
use crate::csg::EvalMode;
use crate::dc::{Mesh, DC_MARGIN_CELLS};

/// The 12 edges of a cell as (corner_a, corner_b) indices into the
/// 8-corner cube, corner index = dx | dy<<1 | dz<<2.
const CUBE_EDGES: [(usize, usize); 12] = [
    (0, 1),
    (2, 3),
    (4, 5),
    (6, 7), // x-aligned
    (0, 2),
    (1, 3),
    (4, 6),
    (5, 7), // y-aligned
    (0, 4),
    (1, 5),
    (2, 6),
    (3, 7), // z-aligned
];

/// Corner offset (dx, dy, dz) for each of the 8 cube corners.
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

/// Mesh a body with dense uniform Surface Nets at `resolution` cells
/// along the longest AABB axis. Empty scene → empty mesh.
pub fn surface_nets_builtin(node: &BoundedCsg, resolution: u32) -> Mesh {
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
    let (cx, cy, _cz) = (nx + 1, ny + 1, nz + 1);

    let corner_pos = |x: usize, y: usize, z: usize| -> [f32; 3] {
        [
            gmin[0] + x as f32 * cell,
            gmin[1] + y as f32 * cell,
            gmin[2] + z as f32 * cell,
        ]
    };
    let corner_idx = |x: usize, y: usize, z: usize| -> usize { (z * cy + y) * cx + x };

    // 1. Sample the field at every corner.
    let mut values = vec![0.0f32; cx * cy * (nz + 1)];
    for z in 0..=nz {
        for y in 0..=ny {
            for x in 0..=nx {
                values[corner_idx(x, y, z)] = node.sdf(corner_pos(x, y, z), EvalMode::Visual);
            }
        }
    }

    // 2. One dual vertex per sign-changing cell.
    let cell_idx = |x: usize, y: usize, z: usize| -> usize { (z * ny + y) * nx + x };
    let mut cell_vertex = vec![u32::MAX; nx * ny * nz];
    let mut positions: Vec<f32> = Vec::new();

    for z in 0..nz {
        for y in 0..ny {
            for x in 0..nx {
                let mut corner_v = [0.0f32; 8];
                let mut has_neg = false;
                let mut has_pos = false;
                for (c, off) in CUBE_CORNERS.iter().enumerate() {
                    let v = values[corner_idx(x + off[0], y + off[1], z + off[2])];
                    corner_v[c] = v;
                    if v < 0.0 {
                        has_neg = true;
                    } else {
                        has_pos = true;
                    }
                }
                if !(has_neg && has_pos) {
                    continue;
                }

                let mut sum = [0.0f32; 3];
                let mut count = 0.0f32;
                for &(a, b) in CUBE_EDGES.iter() {
                    let va = corner_v[a];
                    let vb = corner_v[b];
                    if (va < 0.0) == (vb < 0.0) {
                        continue;
                    }
                    let t = va / (va - vb);
                    let oa = CUBE_CORNERS[a];
                    let ob = CUBE_CORNERS[b];
                    let pa = corner_pos(x + oa[0], y + oa[1], z + oa[2]);
                    let pb = corner_pos(x + ob[0], y + ob[1], z + ob[2]);
                    for d in 0..3 {
                        sum[d] += pa[d] + t * (pb[d] - pa[d]);
                    }
                    count += 1.0;
                }
                if count == 0.0 {
                    continue;
                }
                let mut p = [sum[0] / count, sum[1] / count, sum[2] / count];
                project_to_surface(node, &mut p, cell);

                cell_vertex[cell_idx(x, y, z)] = (positions.len() / 3) as u32;
                positions.extend_from_slice(&p);
            }
        }
    }

    if positions.is_empty() {
        return Mesh::default();
    }

    // 3. Emit a quad per interior sign-changing grid edge.
    let mut indices: Vec<u32> = Vec::new();
    let v_at = |cv: &[u32], x: usize, y: usize, z: usize| -> u32 { cv[cell_idx(x, y, z)] };

    for z in 1..nz {
        for y in 1..ny {
            for x in 0..nx {
                let a = values[corner_idx(x, y, z)];
                let b = values[corner_idx(x + 1, y, z)];
                if (a < 0.0) == (b < 0.0) {
                    continue;
                }
                let q = [
                    v_at(&cell_vertex, x, y - 1, z - 1),
                    v_at(&cell_vertex, x, y, z - 1),
                    v_at(&cell_vertex, x, y, z),
                    v_at(&cell_vertex, x, y - 1, z),
                ];
                push_quad(&mut indices, q, a < 0.0);
            }
        }
    }
    for z in 1..nz {
        for y in 0..ny {
            for x in 1..nx {
                let a = values[corner_idx(x, y, z)];
                let b = values[corner_idx(x, y + 1, z)];
                if (a < 0.0) == (b < 0.0) {
                    continue;
                }
                let q = [
                    v_at(&cell_vertex, x - 1, y, z - 1),
                    v_at(&cell_vertex, x, y, z - 1),
                    v_at(&cell_vertex, x, y, z),
                    v_at(&cell_vertex, x - 1, y, z),
                ];
                push_quad(&mut indices, q, b < 0.0);
            }
        }
    }
    for z in 0..nz {
        for y in 1..ny {
            for x in 1..nx {
                let a = values[corner_idx(x, y, z)];
                let b = values[corner_idx(x, y, z + 1)];
                if (a < 0.0) == (b < 0.0) {
                    continue;
                }
                let q = [
                    v_at(&cell_vertex, x - 1, y - 1, z),
                    v_at(&cell_vertex, x, y - 1, z),
                    v_at(&cell_vertex, x, y, z),
                    v_at(&cell_vertex, x - 1, y, z),
                ];
                push_quad(&mut indices, q, a < 0.0);
            }
        }
    }

    // 4. Per-vertex normals from the SDF gradient.
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

/// Split a dual quad into two triangles (winding by the crossing
/// direction; `fix_orientation` corrects any residual inconsistency).
fn push_quad(indices: &mut Vec<u32>, q: [u32; 4], flip: bool) {
    if q.iter().any(|&v| v == u32::MAX) {
        return;
    }
    let (a, b, c, d) = if flip {
        (q[0], q[3], q[2], q[1])
    } else {
        (q[0], q[1], q[2], q[3])
    };
    indices.extend_from_slice(&[a, b, c, a, c, d]);
}

/// A few Newton steps onto the SDF zero-set, clamped to the cell.
fn project_to_surface(node: &BoundedCsg, p: &mut [f32; 3], cell: f32) {
    const ITERS: usize = 3;
    let max_move = cell * 0.75;
    for _ in 0..ITERS {
        let d = node.sdf(*p, EvalMode::Visual);
        if d.abs() < 1e-6 {
            break;
        }
        let g = crate::dc::sdf_normal(node, *p, cell * 0.25);
        let step = d.clamp(-max_move, max_move);
        p[0] -= step * g[0];
        p[1] -= step * g[1];
        p[2] -= step * g[2];
    }
}
