//! Shared uniform-grid dual contouring for the built-in generic meshers.
//!
//! One dual vertex per sign-changing cell on a uniform grid, one quad per
//! interior sign-changing grid edge → watertight by construction (no
//! T-junctions). Nothing is pruned, so the topology of a non-1-Lipschitz
//! field is captured reliably. The two placements differ only in where
//! each cell's vertex lands:
//!
//! * [`Placement::Centroid`] — the average of the cell's edge crossings,
//!   then a few Newton steps onto the isosurface. Smooth; rounds sharp
//!   edges. This is the built-in **Surface Nets** mesher.
//! * [`Placement::Qef`] — minimise the summed squared distance to the
//!   crossing tangent planes (Hermite data: crossing point + SDF gradient
//!   normal), reusing [`crate::mdc::QefData`]. Preserves sharp feature
//!   edges. This is the built-in **Dual Contouring** mesher. Crossings are
//!   regula-falsi-refined on the exact field (not a single linear guess),
//!   so the zero is located correctly even where the field isn't
//!   1-Lipschitz.
//!
//! Both feed the export pass's decimation + validation like the other
//! dense meshers.

use crate::bvh::BoundedCsg;
use crate::csg::EvalMode;
use crate::dc::{Mesh, DC_MARGIN_CELLS};

/// How a cell's dual vertex is placed.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    /// Average of edge crossings + Newton projection (Surface Nets).
    Centroid,
    /// QEF minimiser of the crossing tangent planes (Dual Contouring).
    Qef,
}

/// Built-in Surface Nets (centroid placement).
pub fn surface_nets_builtin(node: &BoundedCsg, resolution: u32) -> Mesh {
    contour_uniform(node, resolution, Placement::Centroid)
}

/// Built-in feature-preserving Dual Contouring (QEF placement).
pub fn dual_contour_uniform(node: &BoundedCsg, resolution: u32) -> Mesh {
    contour_uniform(node, resolution, Placement::Qef)
}

/// The 12 edges of a cell as (corner_a, corner_b) into the 8-corner cube,
/// corner index = dx | dy<<1 | dz<<2.
const CUBE_EDGES: [(usize, usize); 12] = [
    (0, 1),
    (2, 3),
    (4, 5),
    (6, 7),
    (0, 2),
    (1, 3),
    (4, 6),
    (5, 7),
    (0, 4),
    (1, 5),
    (2, 6),
    (3, 7),
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

pub fn contour_uniform(node: &BoundedCsg, resolution: u32, placement: Placement) -> Mesh {
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
                let mut qef = crate::mdc::QefData::new();
                for &(a, b) in CUBE_EDGES.iter() {
                    let va = corner_v[a];
                    let vb = corner_v[b];
                    if (va < 0.0) == (vb < 0.0) {
                        continue;
                    }
                    let oa = CUBE_CORNERS[a];
                    let ob = CUBE_CORNERS[b];
                    let pa = corner_pos(x + oa[0], y + oa[1], z + oa[2]);
                    let pb = corner_pos(x + ob[0], y + ob[1], z + ob[2]);
                    let cross = match placement {
                        Placement::Centroid => {
                            let t = va / (va - vb);
                            [
                                pa[0] + t * (pb[0] - pa[0]),
                                pa[1] + t * (pb[1] - pa[1]),
                                pa[2] + t * (pb[2] - pa[2]),
                            ]
                        }
                        // Locate the zero on the true field, then feed the
                        // crossing + its gradient normal to the QEF.
                        Placement::Qef => refine_crossing(node, pa, va, pb, vb),
                    };
                    if placement == Placement::Qef {
                        let n = crate::dc::sdf_normal(node, cross, cell * 0.25);
                        qef.add(cross, n);
                    }
                    for d in 0..3 {
                        sum[d] += cross[d];
                    }
                    count += 1.0;
                }
                if count == 0.0 {
                    continue;
                }
                let centroid = [sum[0] / count, sum[1] / count, sum[2] / count];
                let p = match placement {
                    Placement::Centroid => {
                        let mut p = centroid;
                        project_to_surface(node, &mut p, cell);
                        p
                    }
                    Placement::Qef => {
                        // Keep the QEF minimiser unless it flew far outside
                        // its cell (an ill-conditioned near-planar set) — then
                        // fall back to the centroid. Do NOT hard-clamp onto the
                        // cell faces: that would snap neighbouring cells' points
                        // onto the shared boundary and weld them into degenerate
                        // (zero-area) dual quads.
                        let cc = [
                            0.5 * (corner_pos(x, y, z)[0] + corner_pos(x + 1, y + 1, z + 1)[0]),
                            0.5 * (corner_pos(x, y, z)[1] + corner_pos(x + 1, y + 1, z + 1)[1]),
                            0.5 * (corner_pos(x, y, z)[2] + corner_pos(x + 1, y + 1, z + 1)[2]),
                        ];
                        qef_or_centroid(qef.solve(), cc, cell, centroid)
                    }
                };

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

/// Regula-falsi refine the zero crossing between corner `pa` (value `va`)
/// and `pb` (value `vb`, opposite sign) on the exact field — accuracy
/// independent of the field's Lipschitz constant.
fn refine_crossing(
    node: &BoundedCsg,
    pa: [f32; 3],
    va: f32,
    pb: [f32; 3],
    vb: f32,
) -> [f32; 3] {
    let (mut a, mut fa, mut b, mut fb) = (pa, va, pb, vb);
    let lerp = |a: [f32; 3], b: [f32; 3], t: f32| {
        [
            a[0] + t * (b[0] - a[0]),
            a[1] + t * (b[1] - a[1]),
            a[2] + t * (b[2] - a[2]),
        ]
    };
    for _ in 0..5 {
        let denom = fa - fb;
        let t = if denom.abs() < 1e-20 { 0.5 } else { fa / denom };
        let m = lerp(a, b, t.clamp(0.0, 1.0));
        let fm = node.sdf(m, EvalMode::Visual);
        if fm.abs() < 1e-6 {
            return m;
        }
        if (fm < 0.0) == (fa < 0.0) {
            a = m;
            fa = fm;
        } else {
            b = m;
            fb = fm;
        }
    }
    let denom = fa - fb;
    let t = if denom.abs() < 1e-20 { 0.5 } else { fa / denom };
    lerp(a, b, t.clamp(0.0, 1.0))
}

/// Keep the QEF minimiser `p` unless it is more than one cell away from
/// the cell centre `cc` (ill-conditioned); then use `fallback` (the
/// centroid). No hard clamp — see the call site.
fn qef_or_centroid(p: [f32; 3], cc: [f32; 3], cell: f32, fallback: [f32; 3]) -> [f32; 3] {
    let d2 = (p[0] - cc[0]).powi(2) + (p[1] - cc[1]).powi(2) + (p[2] - cc[2]).powi(2);
    if d2 > (cell * 1.0) * (cell * 1.0) {
        fallback
    } else {
        p
    }
}
