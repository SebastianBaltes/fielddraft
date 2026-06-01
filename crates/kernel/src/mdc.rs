//! Manifold Dual Contouring (MDC).
//!
//! Guarantees 2-manifold output by using Nielson's Dual Marching Cubes
//! approach: each cell may contain multiple contour components (cycles),
//! each receiving its own vertex placed via QEF minimization for sharp
//! feature preservation.
//!
//! Reference: Schaefer, Ju, Warren — "Manifold Dual Contouring" (TVCG).

use crate::bvh::BoundedCsg;
use crate::csg::{EvalMode, ProxyElement, MAX_CONTRIBUTORS};
use crate::dc::{sdf_normal, Mesh, CONTRIBUTOR_SENTINEL};
use std::collections::HashMap;

// ── MC vertex / edge conventions ────────────────────────────────────
//
//   MC vertex layout (Paul Bourke / Lorensen & Cline):
//
//       7-----------6
//      /|          /|
//     / |         / |
//    4-----------5  |
//    |  3--------|--2
//    | /         | /
//    |/          |/
//    0-----------1
//
//   Vertex positions:
//     0=(0,0,0)  1=(1,0,0)  2=(1,1,0)  3=(0,1,0)
//     4=(0,0,1)  5=(1,0,1)  6=(1,1,1)  7=(0,1,1)

const MC_CORNER_POS: [[f32; 3]; 8] = [
    [0.0, 0.0, 0.0], // 0
    [1.0, 0.0, 0.0], // 1
    [1.0, 1.0, 0.0], // 2
    [0.0, 1.0, 0.0], // 3
    [0.0, 0.0, 1.0], // 4
    [1.0, 0.0, 1.0], // 5
    [1.0, 1.0, 1.0], // 6
    [0.0, 1.0, 1.0], // 7
];

const MC_EDGE_VERTS: [[usize; 2]; 12] = [
    [0, 1], [1, 2], [2, 3], [3, 0], // bottom ring
    [4, 5], [5, 6], [6, 7], [7, 4], // top ring
    [0, 4], [1, 5], [2, 6], [3, 7], // vertical pillars
];

/// Maps each MC edge of a cell to the grid edge it corresponds to.
/// (axis, di, dj, dk) — grid edge at (ci+di, cj+dj, ck+dk) along axis.
#[allow(dead_code)]
const MC_EDGE_TO_GRID: [(usize, usize, usize, usize); 12] = [
    (0, 0, 0, 0), // MC 0:  X-edge at (ci,   cj,   ck)
    (1, 1, 0, 0), // MC 1:  Y-edge at (ci+1, cj,   ck)
    (0, 0, 1, 0), // MC 2:  X-edge at (ci,   cj+1, ck)
    (1, 0, 0, 0), // MC 3:  Y-edge at (ci,   cj,   ck)
    (0, 0, 0, 1), // MC 4:  X-edge at (ci,   cj,   ck+1)
    (1, 1, 0, 1), // MC 5:  Y-edge at (ci+1, cj,   ck+1)
    (0, 0, 1, 1), // MC 6:  X-edge at (ci,   cj+1, ck+1)
    (1, 0, 0, 1), // MC 7:  Y-edge at (ci,   cj,   ck+1)
    (2, 0, 0, 0), // MC 8:  Z-edge at (ci,   cj,   ck)
    (2, 1, 0, 0), // MC 9:  Z-edge at (ci+1, cj,   ck)
    (2, 1, 1, 0), // MC 10: Z-edge at (ci+1, cj+1, ck)
    (2, 0, 1, 0), // MC 11: Z-edge at (ci,   cj+1, ck)
];

/// For each grid-edge axis, the 4 (cell-offset, mc-edge) pairs that
/// share that edge, in quad winding order (v00, v10, v11, v01).
///
/// For axis A grid-edge at (ei, ej, ek), cell is at
/// (ei + off[0], ej + off[1], ek + off[2]).
const QUAD_CELLS: [[(i32, i32, i32, usize); 4]; 3] = [
    // X-edges: 4 cells in the YZ plane
    [
        (0, -1, -1, 6), // v00
        (0,  0, -1, 4), // v10
        (0,  0,  0, 0), // v11
        (0, -1,  0, 2), // v01
    ],
    // Y-edges: 4 cells in the XZ plane
    [
        (-1, 0, -1, 5), // v00
        ( 0, 0, -1, 7), // v10
        ( 0, 0,  0, 3), // v11
        (-1, 0,  0, 1), // v01
    ],
    // Z-edges: 4 cells in the XY plane
    [
        (-1, -1, 0, 10), // v00
        ( 0, -1, 0, 11), // v10
        ( 0,  0, 0,  8), // v11
        (-1,  0, 0,  9), // v01
    ],
];

use fielddraft_protocol::mc_tables::{EDGE_TABLE, TRI_TABLE};

// ── QEF Solver ──────────────────────────────────────────────────────

struct QefData {
    ata: [f64; 6],
    atb: [f64; 3],
    btb: f64,
    mass_point: [f64; 3],
    num_points: u32,
}

impl QefData {
    fn new() -> Self {
        Self {
            ata: [0.0; 6],
            atb: [0.0; 3],
            btb: 0.0,
            mass_point: [0.0; 3],
            num_points: 0,
        }
    }

    fn add(&mut self, p: [f32; 3], n: [f32; 3]) {
        let (px, py, pz) = (p[0] as f64, p[1] as f64, p[2] as f64);
        let (nx, ny, nz) = (n[0] as f64, n[1] as f64, n[2] as f64);

        self.ata[0] += nx * nx;
        self.ata[1] += nx * ny;
        self.ata[2] += nx * nz;
        self.ata[3] += ny * ny;
        self.ata[4] += ny * nz;
        self.ata[5] += nz * nz;

        let dot = nx * px + ny * py + nz * pz;
        self.atb[0] += nx * dot;
        self.atb[1] += ny * dot;
        self.atb[2] += nz * dot;
        self.btb += dot * dot;

        self.mass_point[0] += px;
        self.mass_point[1] += py;
        self.mass_point[2] += pz;
        self.num_points += 1;
    }

    fn solve(&self) -> [f32; 3] {
        if self.num_points == 0 {
            return [0.0; 3];
        }
        let mp = [
            self.mass_point[0] / self.num_points as f64,
            self.mass_point[1] / self.num_points as f64,
            self.mass_point[2] / self.num_points as f64,
        ];
        let solved = svd_solve_3x3(&self.ata, &self.atb, &mp);
        [solved[0] as f32, solved[1] as f32, solved[2] as f32]
    }
}

fn svd_solve_3x3(ata: &[f64; 6], atb: &[f64; 3], mass_point: &[f64; 3]) -> [f64; 3] {
    let mut a = [
        [ata[0], ata[1], ata[2]],
        [ata[1], ata[3], ata[4]],
        [ata[2], ata[4], ata[5]],
    ];

    let b = [
        atb[0] - (a[0][0] * mass_point[0] + a[0][1] * mass_point[1] + a[0][2] * mass_point[2]),
        atb[1] - (a[1][0] * mass_point[0] + a[1][1] * mass_point[1] + a[1][2] * mass_point[2]),
        atb[2] - (a[2][0] * mass_point[0] + a[2][1] * mass_point[1] + a[2][2] * mass_point[2]),
    ];

    let mut v = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    for _ in 0..10 {
        for p in 0..3 {
            for q in (p + 1)..3 {
                jacobi_rotation(&mut a, &mut v, p, q);
            }
        }
    }

    let threshold = 0.1;
    let mut x = [0.0f64; 3];
    for i in 0..3 {
        let sigma = a[i][i];
        if sigma.abs() > threshold {
            let vb = v[0][i] * b[0] + v[1][i] * b[1] + v[2][i] * b[2];
            let contrib = vb / sigma;
            x[0] += v[0][i] * contrib;
            x[1] += v[1][i] * contrib;
            x[2] += v[2][i] * contrib;
        }
    }

    [
        x[0] + mass_point[0],
        x[1] + mass_point[1],
        x[2] + mass_point[2],
    ]
}

fn jacobi_rotation(a: &mut [[f64; 3]; 3], v: &mut [[f64; 3]; 3], p: usize, q: usize) {
    if a[p][q].abs() < 1e-15 {
        return;
    }
    let tau = (a[q][q] - a[p][p]) / (2.0 * a[p][q]);
    let t = if tau >= 0.0 {
        1.0 / (tau + (1.0 + tau * tau).sqrt())
    } else {
        -1.0 / (-tau + (1.0 + tau * tau).sqrt())
    };
    let c = 1.0 / (1.0 + t * t).sqrt();
    let s = t * c;

    let app = a[p][p];
    let aqq = a[q][q];
    let apq = a[p][q];
    a[p][p] = c * c * app - 2.0 * s * c * apq + s * s * aqq;
    a[q][q] = s * s * app + 2.0 * s * c * apq + c * c * aqq;
    a[p][q] = 0.0;
    a[q][p] = 0.0;
    // r indexes a[r][p], a[p][r], a[r][q], a[q][r] — multi-position, not single-slice
    #[allow(clippy::needless_range_loop)]
    for r in 0..3 {
        if r != p && r != q {
            let arp = a[r][p];
            let arq = a[r][q];
            a[r][p] = c * arp - s * arq;
            a[p][r] = a[r][p];
            a[r][q] = s * arp + c * arq;
            a[q][r] = a[r][q];
        }
    }

    for row in v.iter_mut().take(3) {
        let vrp = row[p];
        let vrq = row[q];
        row[p] = c * vrp - s * vrq;
        row[q] = s * vrp + c * vrq;
    }
}

// ── Cycle detection ─────────────────────────────────────────────────

/// For a given MC case, determine which edges belong to which contour
/// cycle using union-find on the MC triangle table.
///
/// Returns (edge_to_cycle, num_cycles) where edge_to_cycle[e] is the
/// cycle index for MC edge e, or -1 if the edge has no sign change.
fn compute_edge_cycles(case: u8) -> ([i8; 12], usize) {
    let mut parent = [0u8; 12];
    for (i, p) in parent.iter_mut().enumerate() {
        *p = i as u8;
    }

    fn find(parent: &mut [u8; 12], mut x: u8) -> u8 {
        while parent[x as usize] != x {
            parent[x as usize] = parent[parent[x as usize] as usize];
            x = parent[x as usize];
        }
        x
    }

    fn union(parent: &mut [u8; 12], a: u8, b: u8) {
        let ra = find(parent, a);
        let rb = find(parent, b);
        if ra != rb {
            parent[ra as usize] = rb;
        }
    }

    let tris = &TRI_TABLE[case as usize];
    let mut active = [false; 12];
    let mut i = 0;
    while i + 2 < 16 && tris[i] >= 0 {
        let e0 = tris[i] as u8;
        let e1 = tris[i + 1] as u8;
        let e2 = tris[i + 2] as u8;
        active[e0 as usize] = true;
        active[e1 as usize] = true;
        active[e2 as usize] = true;
        union(&mut parent, e0, e1);
        union(&mut parent, e1, e2);
        i += 3;
    }

    let mut cycle_of_root = [-1i8; 12];
    let mut next_cycle = 0i8;
    let mut edge_cycle = [-1i8; 12];

    for e in 0..12 {
        if !active[e] {
            continue;
        }
        let root = find(&mut parent, e as u8) as usize;
        if cycle_of_root[root] < 0 {
            cycle_of_root[root] = next_cycle;
            next_cycle += 1;
        }
        edge_cycle[e] = cycle_of_root[root];
    }

    (edge_cycle, next_cycle as usize)
}

// ── Main algorithm ──────────────────────────────────────────────────

struct MdcVertex {
    pos: [f32; 3],
    normal: [f32; 3],
    classification: u8,
    contributors: [u32; MAX_CONTRIBUTORS],
}

/// Per-cell data: maps each MC edge to the vertex index assigned to it.
struct CellInfo {
    edge_vertex: [u32; 12],
}

pub fn manifold_dc(
    node: &BoundedCsg,
    grid_min: [f32; 3],
    cell: f32,
    nx: usize,
    ny: usize,
    nz: usize,
) -> Mesh {
    let cnx = nx + 1;
    let cny = ny + 1;
    let cnz = nz + 1;

    let corner_index =
        |i: usize, j: usize, k: usize| -> usize { i + j * cnx + k * cnx * cny };
    let corner_point = |i: usize, j: usize, k: usize| -> [f32; 3] {
        [
            grid_min[0] + i as f32 * cell,
            grid_min[1] + j as f32 * cell,
            grid_min[2] + k as f32 * cell,
        ]
    };
    let cell_index = |i: usize, j: usize, k: usize| -> usize { i + j * nx + k * nx * ny };

    // ── 1. Evaluate SDF at all grid corners ─────────────────────────

    let mut sdf_grid = vec![0.0_f32; cnx * cny * cnz];
    for k in 0..cnz {
        for j in 0..cny {
            for i in 0..cnx {
                sdf_grid[corner_index(i, j, k)] =
                    node.sdf(corner_point(i, j, k), EvalMode::Visual);
            }
        }
    }

    let normal_eps = cell * 0.25;
    let contributor_eps = cell * 0.5;

    // ── 2. Process cells: MC case → cycles → QEF → vertices ────────

    let mut vertices: Vec<MdcVertex> = Vec::new();
    let mut cell_data: HashMap<usize, CellInfo> = HashMap::new();

    for ck in 0..nz {
        for cj in 0..ny {
            for ci in 0..nx {
                // Get SDF values at 8 MC corners of this cell
                let mc_corners: [f32; 8] = std::array::from_fn(|v| {
                    let gi = ci + MC_CORNER_POS[v][0] as usize;
                    let gj = cj + MC_CORNER_POS[v][1] as usize;
                    let gk = ck + MC_CORNER_POS[v][2] as usize;
                    sdf_grid[corner_index(gi, gj, gk)]
                });

                // Compute MC case index
                let mut case = 0u8;
                for (v, corner) in mc_corners.iter().enumerate() {
                    if *corner < 0.0 {
                        case |= 1 << v;
                    }
                }

                if EDGE_TABLE[case as usize] == 0 {
                    continue;
                }

                // Determine cycles
                let (edge_cycle, num_cycles) = compute_edge_cycles(case);

                let mut edge_vertex = [u32::MAX; 12];
                let cell_min = corner_point(ci, cj, ck);

                for cycle in 0..num_cycles {
                    let cycle_id = cycle as i8;
                    let mut qef = QefData::new();

                    for e in 0..12 {
                        if edge_cycle[e] != cycle_id {
                            continue;
                        }
                        let [v0, v1] = MC_EDGE_VERTS[e];
                        let d0 = mc_corners[v0];
                        let d1 = mc_corners[v1];
                        if (d0 < 0.0) == (d1 < 0.0) {
                            continue;
                        }
                        let p0 = [
                            cell_min[0] + MC_CORNER_POS[v0][0] * cell,
                            cell_min[1] + MC_CORNER_POS[v0][1] * cell,
                            cell_min[2] + MC_CORNER_POS[v0][2] * cell,
                        ];
                        let p1 = [
                            cell_min[0] + MC_CORNER_POS[v1][0] * cell,
                            cell_min[1] + MC_CORNER_POS[v1][1] * cell,
                            cell_min[2] + MC_CORNER_POS[v1][2] * cell,
                        ];
                        let sign_lo = d0 < 0.0;
                        let mut lo = 0.0f32;
                        let mut hi = 1.0f32;
                        let mut t = d0 / (d0 - d1);
                        for _ in 0..6 {
                            let mp = [
                                p0[0] + t * (p1[0] - p0[0]),
                                p0[1] + t * (p1[1] - p0[1]),
                                p0[2] + t * (p1[2] - p0[2]),
                            ];
                            let d = node.sdf(mp, EvalMode::Visual);
                            if (d < 0.0) == sign_lo {
                                lo = t;
                            } else {
                                hi = t;
                            }
                            t = (lo + hi) * 0.5;
                        }
                        let p = [
                            p0[0] + t * (p1[0] - p0[0]),
                            p0[1] + t * (p1[1] - p0[1]),
                            p0[2] + t * (p1[2] - p0[2]),
                        ];
                        let n = sdf_normal(node, p, normal_eps);
                        qef.add(p, n);
                    }

                    if qef.num_points == 0 {
                        continue;
                    }

                    let mut pos = qef.solve();

                    // Clamp to cell bounds
                    let cell_max = [
                        cell_min[0] + cell,
                        cell_min[1] + cell,
                        cell_min[2] + cell,
                    ];
                    pos[0] = pos[0].clamp(cell_min[0], cell_max[0]);
                    pos[1] = pos[1].clamp(cell_min[1], cell_max[1]);
                    pos[2] = pos[2].clamp(cell_min[2], cell_max[2]);

                    let normal = sdf_normal(node, pos, normal_eps);

                    let (_, cs) = node.evaluate(pos, contributor_eps, EvalMode::Visual);
                    // Mesh-level proxy works at feature granularity; the
                    // per-face sub-ids stay in the analytic B-Rep path.
                    let cs = cs.feature_collapsed();
                    let mut element = cs.element();
                    if element == ProxyElement::None {
                        element = ProxyElement::Face;
                    }
                    let slice = cs.as_slice();
                    let mut contribs = [CONTRIBUTOR_SENTINEL; MAX_CONTRIBUTORS];
                    for slot in 0..MAX_CONTRIBUTORS {
                        if slot < slice.len() {
                            contribs[slot] = slice[slot].feature_id();
                        }
                    }

                    let idx = vertices.len() as u32;
                    vertices.push(MdcVertex {
                        pos,
                        normal,
                        classification: element.tag(),
                        contributors: contribs,
                    });

                    // Map each edge of this cycle to this vertex
                    for e in 0..12 {
                        if edge_cycle[e] == cycle_id {
                            edge_vertex[e] = idx;
                        }
                    }
                }

                cell_data.insert(cell_index(ci, cj, ck), CellInfo { edge_vertex });
            }
        }
    }

    // ── 3. Emit quads ───────────────────────────────────────────────

    let mut indices: Vec<u32> = Vec::new();

    // axis drives many match arms (geometry per axis), not just QUAD_CELLS indexing
    #[allow(clippy::needless_range_loop)]
    for axis in 0..3usize {
        let (ei_max, _ej_max, _ek_max) = match axis {
            0 => (nx, ny + 1, nz + 1),
            1 => (nx + 1, ny, nz + 1),
            2 => (nx + 1, ny + 1, nz),
            _ => unreachable!(),
        };
        let (ej_start, ek_start) = match axis {
            0 => (1usize, 1usize),
            1 => (0, 1),
            2 => (0, 0),
            _ => unreachable!(),
        };
        let (ej_end, ek_end) = match axis {
            0 => (ny, nz),
            1 => (ny, nz),
            2 => (ny, nz),
            _ => unreachable!(),
        };

        // Iterate only over grid edges whose 4 adjacent cells all exist
        for ek in ek_start..ek_end {
            for ej in ej_start..ej_end {
                for ei in 0..ei_max {
                    // Check sign change on this grid edge
                    let (ca_pos, cb_pos) = match axis {
                        0 => ((ei, ej, ek), (ei + 1, ej, ek)),
                        1 => ((ei, ej, ek), (ei, ej + 1, ek)),
                        2 => ((ei, ej, ek), (ei, ej, ek + 1)),
                        _ => unreachable!(),
                    };
                    let da = sdf_grid[corner_index(ca_pos.0, ca_pos.1, ca_pos.2)];
                    let db = sdf_grid[corner_index(cb_pos.0, cb_pos.1, cb_pos.2)];
                    if (da < 0.0) == (db < 0.0) {
                        continue;
                    }

                    // Look up vertices from the 4 cells sharing this edge
                    let quad_spec = &QUAD_CELLS[axis];
                    let mut vi = [u32::MAX; 4];
                    let mut all_valid = true;

                    for q in 0..4 {
                        let (di, dj, dk, mc_edge) = quad_spec[q];
                        let ci = ei as i32 + di;
                        let cj = ej as i32 + dj;
                        let ck = ek as i32 + dk;
                        if ci < 0 || cj < 0 || ck < 0
                            || ci >= nx as i32
                            || cj >= ny as i32
                            || ck >= nz as i32
                        {
                            all_valid = false;
                            break;
                        }
                        let cidx = cell_index(ci as usize, cj as usize, ck as usize);
                        if let Some(info) = cell_data.get(&cidx) {
                            vi[q] = info.edge_vertex[mc_edge];
                        } else {
                            all_valid = false;
                            break;
                        }
                    }

                    if !all_valid
                        || vi[0] == u32::MAX
                        || vi[1] == u32::MAX
                        || vi[2] == u32::MAX
                        || vi[3] == u32::MAX
                    {
                        continue;
                    }

                    // Skip degenerate quads
                    if vi[0] == vi[1] || vi[0] == vi[2] || vi[0] == vi[3]
                        || vi[1] == vi[2] || vi[1] == vi[3] || vi[2] == vi[3]
                    {
                        continue;
                    }

                    let (v00, v10, v11, v01) = (vi[0], vi[1], vi[2], vi[3]);

                    let flip = axis == 1;
                    if (da > 0.0) ^ flip {
                        indices.extend_from_slice(&[v00, v01, v11]);
                        indices.extend_from_slice(&[v00, v11, v10]);
                    } else {
                        indices.extend_from_slice(&[v00, v10, v11]);
                        indices.extend_from_slice(&[v00, v11, v01]);
                    }
                }
            }
        }
    }

    // ── 4. Pack into Mesh ───────────────────────────────────────────

    let vert_count = vertices.len();
    let mut positions = Vec::with_capacity(vert_count * 3);
    let mut normals = Vec::with_capacity(vert_count * 3);
    let mut classifications = Vec::with_capacity(vert_count);
    let mut contributors = Vec::with_capacity(vert_count * MAX_CONTRIBUTORS);

    for v in &vertices {
        positions.extend_from_slice(&v.pos);
        normals.extend_from_slice(&v.normal);
        classifications.push(v.classification);
        contributors.extend_from_slice(&v.contributors);
    }

    Mesh {
        positions,
        normals,
        indices,
        classifications,
        contributors,
    }
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::csg::CsgNode;
    use fielddraft_protocol::{BoolOp, Feature, FeatureId, Primitive, Transform};

    fn make_feature(id: u32, prim: Primitive) -> Feature {
        Feature {
            id: FeatureId::from_u32(id),
            name: String::new(),
            op: BoolOp::Union,
            primitive: prim,
            transform: Transform::IDENTITY,
            blend_radius: 0.0,
            chamfer_size: 0.0,
            twist_rate: 0.0,
            bend_rate: 0.0,
            displace_depth: 0.0,
            displace_freq: 0.0,
            shell_thickness: 0.0,
            target_ids: vec![],
        }
    }

    fn bounded_sphere(center: [f32; 3], radius: f32) -> crate::bvh::BoundedCsg {
        let f = make_feature(1, Primitive::Sphere { center, radius });
        let csg = CsgNode::from_features(&[f]);
        crate::bvh::BoundedCsg::from_csg(&csg)
    }

    fn bounded_box(center: [f32; 3], half: [f32; 3]) -> crate::bvh::BoundedCsg {
        let f = make_feature(
            1,
            Primitive::Box {
                center,
                half_extents: half,
            },
        );
        let csg = CsgNode::from_features(&[f]);
        crate::bvh::BoundedCsg::from_csg(&csg)
    }

    fn run_manifold_dc(bounded: &crate::bvh::BoundedCsg, resolution: u32) -> Mesh {
        let aabb = bounded.aabb();
        let raw_size = aabb.size();
        let max_dim = raw_size[0].max(raw_size[1]).max(raw_size[2]);
        let cell = max_dim / resolution as f32;
        let grid = aabb.expanded(cell * crate::dc::DC_MARGIN_CELLS);
        let size = grid.size();
        let nx = ((size[0] / cell).ceil() as usize).max(1);
        let ny = ((size[1] / cell).ceil() as usize).max(1);
        let nz = ((size[2] / cell).ceil() as usize).max(1);
        manifold_dc(bounded, grid.min, cell, nx, ny, nz)
    }

    #[test]
    fn cycle_detection_single_triangle() {
        // Case 1: only MC vertex 0 inside → edges 0, 3, 8 → 1 cycle
        let (cycles, num) = compute_edge_cycles(1);
        assert_eq!(num, 1);
        assert!(cycles[0] >= 0);
        assert!(cycles[3] >= 0);
        assert!(cycles[8] >= 0);
        assert_eq!(cycles[0], cycles[3]);
        assert_eq!(cycles[3], cycles[8]);
    }

    #[test]
    fn cycle_detection_empty() {
        let (cycles, num) = compute_edge_cycles(0);
        assert_eq!(num, 0);
        assert!(cycles.iter().all(|&c| c < 0));
    }

    #[test]
    fn cycle_detection_full() {
        let (cycles, num) = compute_edge_cycles(255);
        assert_eq!(num, 0);
        assert!(cycles.iter().all(|&c| c < 0));
    }

    #[test]
    fn sphere_produces_manifold_mesh() {
        let bounded = bounded_sphere([0.0, 0.0, 0.0], 10.0);
        let mesh = run_manifold_dc(&bounded, 32);

        assert!(mesh.vertex_count() > 0, "mesh should have vertices");
        assert!(mesh.triangle_count() > 0, "mesh should have triangles");
        assert_eq!(mesh.indices.len() % 3, 0);

        for &idx in &mesh.indices {
            assert!(
                (idx as usize) < mesh.vertex_count(),
                "index {idx} out of bounds"
            );
        }

        // Check manifold: every edge should be shared by exactly 2 triangles
        let mut edge_count: HashMap<(u32, u32), usize> = HashMap::new();
        for t in 0..mesh.triangle_count() {
            let tri = [
                mesh.indices[t * 3],
                mesh.indices[t * 3 + 1],
                mesh.indices[t * 3 + 2],
            ];
            for e in 0..3 {
                let a = tri[e];
                let b = tri[(e + 1) % 3];
                let key = if a < b { (a, b) } else { (b, a) };
                *edge_count.entry(key).or_default() += 1;
            }
        }
        let non_manifold = edge_count.values().filter(|&&c| c != 2).count();
        assert_eq!(
            non_manifold, 0,
            "{non_manifold} non-manifold edges (expected all edges shared by exactly 2 triangles)"
        );
    }

    #[test]
    fn sphere_vertices_hug_surface() {
        let radius = 10.0;
        let bounded = bounded_sphere([0.0, 0.0, 0.0], radius);
        let mesh = run_manifold_dc(&bounded, 32);

        let mut max_dist = 0.0f32;
        for i in 0..mesh.vertex_count() {
            let x = mesh.positions[i * 3];
            let y = mesh.positions[i * 3 + 1];
            let z = mesh.positions[i * 3 + 2];
            let dist = (x * x + y * y + z * z).sqrt() - radius;
            max_dist = max_dist.max(dist.abs());
        }
        let cell = 20.0 / 32.0;
        assert!(
            max_dist < cell * 2.0,
            "sphere vertices should be near surface, max dist = {max_dist}"
        );
    }

    #[test]
    fn box_preserves_sharp_edges() {
        let half = [5.0, 5.0, 5.0];
        let bounded = bounded_box([0.0, 0.0, 0.0], half);
        let mesh = run_manifold_dc(&bounded, 32);

        assert!(mesh.vertex_count() > 0);
        assert!(mesh.triangle_count() > 0);

        let corners = [
            [-5.0, -5.0, -5.0],
            [5.0, -5.0, -5.0],
            [-5.0, 5.0, -5.0],
            [5.0, 5.0, -5.0],
            [-5.0, -5.0, 5.0],
            [5.0, -5.0, 5.0],
            [-5.0, 5.0, 5.0],
            [5.0, 5.0, 5.0],
        ];
        let mut corners_found = 0;
        for corner in &corners {
            let mut best = f32::MAX;
            for i in 0..mesh.vertex_count() {
                let dx = mesh.positions[i * 3] - corner[0];
                let dy = mesh.positions[i * 3 + 1] - corner[1];
                let dz = mesh.positions[i * 3 + 2] - corner[2];
                let d = (dx * dx + dy * dy + dz * dz).sqrt();
                best = best.min(d);
            }
            if best < 1.0 {
                corners_found += 1;
            }
        }
        assert!(
            corners_found >= 6,
            "QEF should place vertices near box corners, found {corners_found}/8"
        );
    }

    #[test]
    fn box_manifold_check() {
        let bounded = bounded_box([0.0, 0.0, 0.0], [5.0, 5.0, 5.0]);
        let mesh = run_manifold_dc(&bounded, 16);

        assert!(mesh.vertex_count() > 0);

        let mut edge_count: HashMap<(u32, u32), usize> = HashMap::new();
        for t in 0..mesh.triangle_count() {
            let tri = [
                mesh.indices[t * 3],
                mesh.indices[t * 3 + 1],
                mesh.indices[t * 3 + 2],
            ];
            for e in 0..3 {
                let a = tri[e];
                let b = tri[(e + 1) % 3];
                let key = if a < b { (a, b) } else { (b, a) };
                *edge_count.entry(key).or_default() += 1;
            }
        }
        let non_manifold = edge_count.values().filter(|&&c| c != 2).count();
        assert_eq!(non_manifold, 0, "{non_manifold} non-manifold edges in box mesh");
    }

    #[test]
    fn classification_arrays_parallel() {
        let bounded = bounded_box([0.0, 0.0, 0.0], [5.0, 5.0, 5.0]);
        let mesh = run_manifold_dc(&bounded, 16);

        assert_eq!(mesh.classifications.len(), mesh.vertex_count());
        assert_eq!(
            mesh.contributors.len(),
            mesh.vertex_count() * MAX_CONTRIBUTORS
        );
        assert_eq!(mesh.positions.len(), mesh.vertex_count() * 3);
        assert_eq!(mesh.normals.len(), mesh.vertex_count() * 3);
    }

    #[test]
    fn qef_places_vertex_on_plane() {
        let mut qef = QefData::new();
        qef.add([5.0, 0.0, 0.0], [1.0, 0.0, 0.0]);
        qef.add([5.0, 1.0, 0.0], [1.0, 0.0, 0.0]);
        qef.add([5.0, 0.0, 1.0], [1.0, 0.0, 0.0]);
        let pos = qef.solve();
        assert!(
            (pos[0] - 5.0).abs() < 0.1,
            "x should be ~5, got {}",
            pos[0]
        );
    }
}
