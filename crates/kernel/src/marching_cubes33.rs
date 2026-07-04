//! Marching Cubes 33 — Lewiner's topologically-controlled Marching Cubes,
//! plus Extended Marching Cubes (Kobbelt) sharing the same core.
//!
//! Ported from `JuliaGeometry/MarchingCubes.jl` (which itself adapts Thomas
//! Lewiner's reference C++ from JGT 8(2):1-15, 2003). The classical 15-case
//! MC table has ambiguous faces and interior configurations that plain MC
//! resolves arbitrarily, leaving cracks / non-manifold saddles. MC33 tests
//! the trilinear interpolant on each ambiguous face (`test_face`) and cube
//! interior (`test_interior`) and picks a triangulation from an extended
//! 730-subcase table so the result is **manifold and consistent with the
//! trilinear surface** — a watertight, correctly-connected mesh for any field.
//!
//! Vertices live on the grid edges and are shared through `vert_idx` (one
//! canonical vertex per crossing edge), so the mesh is watertight by shared-
//! vertex construction — the same property that makes it a good gyroid mesher.
//!
//! Extended Marching Cubes (`extended_marching_cubes`) reuses this machinery:
//! cells whose crossing normals span a wide angle carry a sharp CSG feature;
//! there we insert a QEF feature vertex (least-squares intersection of the
//! crossing tangent planes, clamped into the cell against spikes) and tent the
//! cell's MC patch onto it — recovering crisp boolean edges the flat MC patch
//! rounds off, while every patch-boundary edge stays shared → still watertight.

use crate::bvh::BoundedCsg;
use crate::csg::EvalMode;
use crate::dc::{sdf_normal, Mesh, DC_MARGIN_CELLS};
use crate::marching_cubes33_tables as t;
use crate::mdc::QefData;

/// Near-zero corner values are nudged to +EPS (matches the reference), so a
/// sample sitting exactly on the isosurface is treated as "outside" both when
/// building the case index and when placing edge crossings — the two must
/// agree or the case would reference a crossing that was never created.
const EPS: f32 = f32::EPSILON;

/// EMC: a cell is a sharp-feature cell when two of its crossing normals differ
/// by more than this angle. ~37° ignores ordinary curvature but catches the
/// ~90° dihedral of a CSG boolean edge.
const EMC_FEATURE_COS: f32 = 0.8;

/// Marching Cubes 33 at `resolution` cells along the longest AABB axis.
pub fn marching_cubes33(node: &BoundedCsg, resolution: u32) -> Mesh {
    run(node, resolution, false)
}

/// Extended Marching Cubes (feature-sensitive) — MC33 base + QEF feature tents.
pub fn extended_marching_cubes(node: &BoundedCsg, resolution: u32) -> Mesh {
    run(node, resolution, true)
}

fn run(node: &BoundedCsg, resolution: u32, emc: bool) -> Mesh {
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
    let (sx, sy, sz) = (nx + 1, ny + 1, nz + 1);

    let mut ctx = Ctx {
        node,
        sx,
        sy,
        sz,
        cell,
        gmin,
        values: vec![0.0; sx * sy * sz],
        vert_idx: vec![-1; 3 * sx * sy * sz],
        positions: Vec::new(),
        indices: Vec::new(),
        cube: [0.0; 8],
        tribuf: [-1; 3],
        cube_tris: Vec::with_capacity(16),
        emc,
    };

    // Sample the field at every grid corner.
    for k in 0..sz {
        for j in 0..sy {
            for i in 0..sx {
                let p = ctx.corner_pos(i, j, k);
                ctx.values[(k * sy + j) * sx + i] = node.sdf(p, EvalMode::Visual);
            }
        }
    }

    ctx.compute_intersection_points();

    for k in 0..(sz - 1) {
        for j in 0..(sy - 1) {
            for i in 0..(sx - 1) {
                ctx.process_cube(i, j, k);
            }
        }
    }

    if ctx.indices.is_empty() {
        return Mesh::default();
    }

    // Per-vertex normals from the SDF gradient (winding is fixed by the export
    // pass's `fix_orientation`, so a global sign flip here is harmless).
    let vcount = ctx.positions.len() / 3;
    let mut normals = vec![0.0f32; ctx.positions.len()];
    for v in 0..vcount {
        let p = [
            ctx.positions[v * 3],
            ctx.positions[v * 3 + 1],
            ctx.positions[v * 3 + 2],
        ];
        let n = sdf_normal(node, p, cell * 0.25);
        normals[v * 3] = n[0];
        normals[v * 3 + 1] = n[1];
        normals[v * 3 + 2] = n[2];
    }

    Mesh {
        positions: ctx.positions,
        normals,
        indices: ctx.indices,
        ..Mesh::default()
    }
}

struct Ctx<'a> {
    node: &'a BoundedCsg,
    sx: usize,
    sy: usize,
    sz: usize,
    cell: f32,
    gmin: [f32; 3],
    values: Vec<f32>,
    /// 3 slots per grid point: 0 = +x edge, 1 = +y edge, 2 = +z edge; -1 = none.
    vert_idx: Vec<i32>,
    positions: Vec<f32>,
    indices: Vec<u32>,
    /// 8 corner values of the current cube in Lewiner (Gray-code) order.
    cube: [f32; 8],
    tribuf: [i32; 3],
    /// Triangles emitted for the current cube (vertex ids), before flush.
    cube_tris: Vec<[u32; 3]>,
    emc: bool,
}

#[inline]
fn adj(c: f32) -> f32 {
    if c.abs() < EPS {
        EPS
    } else {
        c
    }
}

impl<'a> Ctx<'a> {
    #[inline]
    fn corner_pos(&self, i: usize, j: usize, k: usize) -> [f32; 3] {
        [
            self.gmin[0] + i as f32 * self.cell,
            self.gmin[1] + j as f32 * self.cell,
            self.gmin[2] + k as f32 * self.cell,
        ]
    }

    #[inline]
    fn val(&self, i: usize, j: usize, k: usize) -> f32 {
        self.values[(k * self.sy + j) * self.sx + i]
    }

    #[inline]
    fn vslot(&self, axis: usize, i: usize, j: usize, k: usize) -> usize {
        3 * ((k * self.sy + j) * self.sx + i) + axis
    }

    #[inline]
    fn get_vidx(&self, axis: usize, i: usize, j: usize, k: usize) -> i32 {
        self.vert_idx[self.vslot(axis, i, j, k)]
    }

    #[inline]
    fn pos(&self, id: i32) -> [f32; 3] {
        let b = (id as usize) * 3;
        [self.positions[b], self.positions[b + 1], self.positions[b + 2]]
    }

    /// Add an edge-crossing vertex on the `axis` edge out of grid point (i,j,k).
    /// `a`,`b` are the (adjusted) field values at the two endpoints.
    fn add_edge_vertex(&mut self, axis: usize, i: usize, j: usize, k: usize, a: f32, b: f32) {
        let u = a / (a - b);
        let p0 = self.corner_pos(i, j, k);
        let mut p = p0;
        p[axis] += u * self.cell;
        let id = (self.positions.len() / 3) as i32;
        self.positions.extend_from_slice(&p);
        let slot = self.vslot(axis, i, j, k);
        self.vert_idx[slot] = id;
    }

    /// Interpolate one crossing vertex per sign-changing grid edge, shared by
    /// all cells that touch the edge → watertight by construction.
    fn compute_intersection_points(&mut self) {
        for k in 0..self.sz {
            for j in 0..self.sy {
                for i in 0..self.sx {
                    let c0 = adj(self.val(i, j, k));
                    let c1 = if i + 1 < self.sx {
                        adj(self.val(i + 1, j, k))
                    } else {
                        c0
                    };
                    let c2 = if j + 1 < self.sy {
                        adj(self.val(i, j + 1, k))
                    } else {
                        c0
                    };
                    let c3 = if k + 1 < self.sz {
                        adj(self.val(i, j, k + 1))
                    } else {
                        c0
                    };
                    if c0 < 0.0 {
                        if c1 > 0.0 {
                            self.add_edge_vertex(0, i, j, k, c0, c1);
                        }
                        if c2 > 0.0 {
                            self.add_edge_vertex(1, i, j, k, c0, c2);
                        }
                        if c3 > 0.0 {
                            self.add_edge_vertex(2, i, j, k, c0, c3);
                        }
                    } else {
                        if c1 < 0.0 {
                            self.add_edge_vertex(0, i, j, k, c0, c1);
                        }
                        if c2 < 0.0 {
                            self.add_edge_vertex(1, i, j, k, c0, c2);
                        }
                        if c3 < 0.0 {
                            self.add_edge_vertex(2, i, j, k, c0, c3);
                        }
                    }
                }
            }
        }
    }

    /// Cube sign code (0..255) — also fills `self.cube` with the 8 adjusted
    /// corner values in Lewiner Gray-code order.
    fn lut_entry(&mut self, i: usize, j: usize, k: usize) -> usize {
        let mut code = 0usize;
        for p in 0..8usize {
            let ci = i + ((p ^ (p >> 1)) & 1);
            let cj = j + ((p >> 1) & 1);
            let ck = k + ((p >> 2) & 1);
            let c = adj(self.val(ci, cj, ck));
            self.cube[p] = c;
            if c > 0.0 {
                code |= 1 << p;
            }
        }
        code
    }

    /// Resolve an edge code (1..12, Lewiner numbering) to a shared vertex id.
    #[inline]
    fn edge_vertex(&self, tr: i8, i: usize, j: usize, k: usize) -> i32 {
        match tr {
            1 => self.get_vidx(0, i, j, k),
            2 => self.get_vidx(1, i + 1, j, k),
            3 => self.get_vidx(0, i, j + 1, k),
            4 => self.get_vidx(1, i, j, k),
            5 => self.get_vidx(0, i, j, k + 1),
            6 => self.get_vidx(1, i + 1, j, k + 1),
            7 => self.get_vidx(0, i, j + 1, k + 1),
            8 => self.get_vidx(1, i, j, k + 1),
            9 => self.get_vidx(2, i, j, k),
            10 => self.get_vidx(2, i + 1, j, k),
            11 => self.get_vidx(2, i + 1, j + 1, k),
            12 => self.get_vidx(2, i, j + 1, k),
            _ => -1,
        }
    }

    /// Emit `n` triangles from a tiling code list into the current cube buffer.
    /// `v12` is the interior (c-vertex) id used when a code isn't an edge.
    fn add_triangle(&mut self, i: usize, j: usize, k: usize, tri: &[i8], n: usize, v12: i32) {
        for tt in 0..(3 * n) {
            let tr = tri[tt];
            let tv = if (1..=12).contains(&tr) {
                self.edge_vertex(tr, i, j, k)
            } else {
                v12
            };
            self.tribuf[tt % 3] = tv;
            if tt % 3 == 2 {
                let (a, b, c) = (self.tribuf[0], self.tribuf[1], self.tribuf[2]);
                if a >= 0 && b >= 0 && c >= 0 && a != b && b != c && a != c {
                    self.cube_tris.push([a as u32, b as u32, c as u32]);
                }
            }
        }
    }

    /// Interior vertex = mean of the cube's present edge crossings.
    fn add_c_vertex(&mut self, i: usize, j: usize, k: usize) -> i32 {
        let mut sum = [0.0f32; 3];
        let mut cnt = 0.0f32;
        for tr in 1..=12i8 {
            let vi = self.edge_vertex(tr, i, j, k);
            if vi >= 0 {
                let p = self.pos(vi);
                sum[0] += p[0];
                sum[1] += p[1];
                sum[2] += p[2];
                cnt += 1.0;
            }
        }
        if cnt == 0.0 {
            return -1;
        }
        let p = [sum[0] / cnt, sum[1] / cnt, sum[2] / cnt];
        let id = (self.positions.len() / 3) as i32;
        self.positions.extend_from_slice(&p);
        id
    }

    fn process_cube(&mut self, i: usize, j: usize, k: usize) {
        let lut = self.lut_entry(i, j, k);
        self.cube_tris.clear();

        let case = t::CASES[lut][0];
        let cfg_raw = t::CASES[lut][1];
        let cfg = if cfg_raw > 0 { (cfg_raw - 1) as usize } else { 0 };
        let mut subcfg: i32 = 1;

        match case {
            1 => {}
            2 => self.add_triangle(i, j, k, t::TILING1[cfg], 1, -1),
            3 => self.add_triangle(i, j, k, t::TILING2[cfg], 2, -1),
            4 => {
                if test_face(&self.cube, t::TEST3[cfg]) {
                    self.add_triangle(i, j, k, t::TILING3_2[cfg], 4, -1);
                } else {
                    self.add_triangle(i, j, k, t::TILING3_1[cfg], 2, -1);
                }
            }
            5 => {
                if self.test_interior(case, cfg, subcfg, t::TEST4[cfg]) {
                    self.add_triangle(i, j, k, t::TILING4_1[cfg], 2, -1);
                } else {
                    self.add_triangle(i, j, k, t::TILING4_2[cfg], 6, -1);
                }
            }
            6 => self.add_triangle(i, j, k, t::TILING5[cfg], 3, -1),
            7 => {
                if test_face(&self.cube, t::TEST6[cfg][0]) {
                    self.add_triangle(i, j, k, t::TILING6_2[cfg], 5, -1);
                } else if self.test_interior(case, cfg, subcfg, t::TEST6[cfg][1]) {
                    self.add_triangle(i, j, k, t::TILING6_1_1[cfg], 3, -1);
                } else {
                    let v = self.add_c_vertex(i, j, k);
                    self.add_triangle(i, j, k, t::TILING6_1_2[cfg], 9, v);
                }
            }
            8 => {
                if test_face(&self.cube, t::TEST7[cfg][0]) {
                    subcfg += 1;
                }
                if test_face(&self.cube, t::TEST7[cfg][1]) {
                    subcfg += 2;
                }
                if test_face(&self.cube, t::TEST7[cfg][2]) {
                    subcfg += 4;
                }
                match subcfg {
                    1 => self.add_triangle(i, j, k, t::TILING7_1[cfg], 3, -1),
                    2 | 3 => {
                        self.add_triangle(i, j, k, t::TILING7_2[cfg][(subcfg - 2) as usize], 5, -1)
                    }
                    4 => {
                        let v = self.add_c_vertex(i, j, k);
                        self.add_triangle(i, j, k, t::TILING7_3[cfg][0], 9, v);
                    }
                    5 => self.add_triangle(i, j, k, t::TILING7_2[cfg][2], 5, -1),
                    6 | 7 => {
                        let v = self.add_c_vertex(i, j, k);
                        self.add_triangle(i, j, k, t::TILING7_3[cfg][(subcfg - 5) as usize], 9, v);
                    }
                    8 => {
                        if self.test_interior(case, cfg, subcfg, t::TEST7[cfg][3]) {
                            self.add_triangle(i, j, k, t::TILING7_4_2[cfg], 9, -1);
                        } else {
                            self.add_triangle(i, j, k, t::TILING7_4_1[cfg], 5, -1);
                        }
                    }
                    _ => {}
                }
            }
            9 => self.add_triangle(i, j, k, t::TILING8[cfg], 2, -1),
            10 => self.add_triangle(i, j, k, t::TILING9[cfg], 4, -1),
            11 => {
                if test_face(&self.cube, t::TEST10[cfg][0]) {
                    if test_face(&self.cube, t::TEST10[cfg][1]) {
                        self.add_triangle(i, j, k, t::TILING10_1_1_[cfg], 4, -1);
                    } else {
                        let v = self.add_c_vertex(i, j, k);
                        self.add_triangle(i, j, k, t::TILING10_2[cfg], 8, v);
                    }
                } else if test_face(&self.cube, t::TEST10[cfg][1]) {
                    let v = self.add_c_vertex(i, j, k);
                    self.add_triangle(i, j, k, t::TILING10_2_[cfg], 8, v);
                } else if self.test_interior(case, cfg, subcfg, t::TEST10[cfg][2]) {
                    self.add_triangle(i, j, k, t::TILING10_1_1[cfg], 4, -1);
                } else {
                    self.add_triangle(i, j, k, t::TILING10_1_2[cfg], 8, -1);
                }
            }
            12 => self.add_triangle(i, j, k, t::TILING11[cfg], 4, -1),
            13 => {
                if test_face(&self.cube, t::TEST12[cfg][0]) {
                    if test_face(&self.cube, t::TEST12[cfg][1]) {
                        self.add_triangle(i, j, k, t::TILING12_1_1_[cfg], 4, -1);
                    } else {
                        let v = self.add_c_vertex(i, j, k);
                        self.add_triangle(i, j, k, t::TILING12_2[cfg], 8, v);
                    }
                } else if test_face(&self.cube, t::TEST12[cfg][1]) {
                    let v = self.add_c_vertex(i, j, k);
                    self.add_triangle(i, j, k, t::TILING12_2_[cfg], 8, v);
                } else if self.test_interior(case, cfg, subcfg, t::TEST12[cfg][2]) {
                    self.add_triangle(i, j, k, t::TILING12_1_1[cfg], 4, -1);
                } else {
                    self.add_triangle(i, j, k, t::TILING12_1_2[cfg], 8, -1);
                }
            }
            14 => {
                if test_face(&self.cube, t::TEST13[cfg][0]) {
                    subcfg += 1;
                }
                if test_face(&self.cube, t::TEST13[cfg][1]) {
                    subcfg += 2;
                }
                if test_face(&self.cube, t::TEST13[cfg][2]) {
                    subcfg += 4;
                }
                if test_face(&self.cube, t::TEST13[cfg][3]) {
                    subcfg += 8;
                }
                if test_face(&self.cube, t::TEST13[cfg][4]) {
                    subcfg += 16;
                }
                if test_face(&self.cube, t::TEST13[cfg][5]) {
                    subcfg += 32;
                }
                let sc = t::SUBCFG13[(subcfg - 1) as usize];
                if sc == 0 {
                    self.add_triangle(i, j, k, t::TILING13_1[cfg], 4, -1);
                } else if (1..=6).contains(&sc) {
                    self.add_triangle(i, j, k, t::TILING13_2[cfg][(sc - 1) as usize], 6, -1);
                } else if (7..=18).contains(&sc) {
                    let v = self.add_c_vertex(i, j, k);
                    self.add_triangle(i, j, k, t::TILING13_3[cfg][(sc - 7) as usize], 10, v);
                } else if (19..=22).contains(&sc) {
                    let v = self.add_c_vertex(i, j, k);
                    self.add_triangle(i, j, k, t::TILING13_4[cfg][(sc - 19) as usize], 12, v);
                } else if (23..=26).contains(&sc) {
                    subcfg = sc as i32 - 22;
                    if self.test_interior(case, cfg, subcfg, t::TEST13[cfg][5]) {
                        self.add_triangle(
                            i,
                            j,
                            k,
                            t::TILING13_5_1[cfg][(subcfg - 1) as usize],
                            6,
                            -1,
                        );
                    } else {
                        self.add_triangle(
                            i,
                            j,
                            k,
                            t::TILING13_5_2[cfg][(subcfg - 1) as usize],
                            10,
                            -1,
                        );
                    }
                } else if (27..=38).contains(&sc) {
                    let v = self.add_c_vertex(i, j, k);
                    self.add_triangle(i, j, k, t::TILING13_3_[cfg][(sc - 27) as usize], 10, v);
                } else if (39..=44).contains(&sc) {
                    self.add_triangle(i, j, k, t::TILING13_2_[cfg][(sc - 39) as usize], 6, -1);
                } else if sc == 45 {
                    self.add_triangle(i, j, k, t::TILING13_1_[cfg], 4, -1);
                }
            }
            15 => self.add_triangle(i, j, k, t::TILING14[cfg], 4, -1),
            _ => {}
        }

        self.flush_cube(i, j, k);
    }

    /// Push this cube's triangles — plain MC33, or (EMC) tent a feature cell's
    /// patch onto a QEF feature vertex to recover a sharp edge/corner.
    fn flush_cube(&mut self, i: usize, j: usize, k: usize) {
        if self.cube_tris.is_empty() {
            return;
        }
        let tris = std::mem::take(&mut self.cube_tris);
        if self.emc {
            if let Some(fid) = self.feature_vertex(i, j, k, &tris) {
                for tri in &tris {
                    let [a, b, c] = *tri;
                    self.indices.extend_from_slice(&[a, b, fid]);
                    self.indices.extend_from_slice(&[b, c, fid]);
                    self.indices.extend_from_slice(&[c, a, fid]);
                }
                self.cube_tris = tris;
                self.cube_tris.clear();
                return;
            }
        }
        for tri in &tris {
            self.indices.extend_from_slice(tri);
        }
        self.cube_tris = tris;
        self.cube_tris.clear();
    }

    /// If this cube carries a sharp feature, create and return a QEF feature
    /// vertex clamped into the cell; otherwise `None`.
    fn feature_vertex(&mut self, i: usize, j: usize, k: usize, tris: &[[u32; 3]]) -> Option<u32> {
        // Distinct vertices of the cube's patch.
        let mut ids: Vec<u32> = Vec::with_capacity(12);
        for tri in tris {
            for &v in tri {
                if !ids.contains(&v) {
                    ids.push(v);
                }
            }
        }
        if ids.len() < 3 {
            return None;
        }
        let normals: Vec<[f32; 3]> = ids
            .iter()
            .map(|&id| sdf_normal(self.node, self.pos(id as i32), self.cell * 0.25))
            .collect();
        // Feature detection: widest pairwise normal angle.
        let mut min_dot = 1.0f32;
        for a in 0..normals.len() {
            for b in (a + 1)..normals.len() {
                let d = normals[a][0] * normals[b][0]
                    + normals[a][1] * normals[b][1]
                    + normals[a][2] * normals[b][2];
                min_dot = min_dot.min(d);
            }
        }
        if min_dot >= EMC_FEATURE_COS {
            return None;
        }
        // QEF: least-squares intersection of the crossing tangent planes.
        let mut qef = QefData::new();
        for (idx, &id) in ids.iter().enumerate() {
            qef.add(self.pos(id as i32), normals[idx]);
        }
        let mut f = qef.solve();
        // Clamp into the cell (with a tiny margin) — the standard guard that
        // stops an ill-conditioned QEF from shooting a spike out of the cell.
        let lo = self.corner_pos(i, j, k);
        let hi = self.corner_pos(i + 1, j + 1, k + 1);
        for a in 0..3 {
            f[a] = f[a].clamp(lo[a], hi[a]);
        }
        let fid = (self.positions.len() / 3) as u32;
        self.positions.extend_from_slice(&f);
        Some(fid)
    }

    /// Port of Lewiner's interior test (trilinear body diagonal / edge sweep).
    /// `s` is the reference sign from the test tables; only its sign matters.
    fn test_interior(&self, case: i8, cfg: usize, subcfg: i32, s: i8) -> bool {
        let cb = &self.cube;
        let at: f32;
        let bt: f32;
        let ct: f32;
        let dt: f32;
        if case == 5 || case == 11 {
            let a = (cb[4] - cb[0]) * (cb[6] - cb[2]) - (cb[7] - cb[3]) * (cb[5] - cb[1]);
            let b = cb[2] * (cb[4] - cb[0]) + cb[0] * (cb[6] - cb[2])
                - cb[1] * (cb[7] - cb[3])
                - cb[3] * (cb[5] - cb[1]);
            let tt = -b / (2.0 * a);
            if tt < 0.0 || tt > 1.0 {
                return s > 0;
            }
            at = cb[0] + (cb[4] - cb[0]) * tt;
            bt = cb[3] + (cb[7] - cb[3]) * tt;
            ct = cb[2] + (cb[6] - cb[2]) * tt;
            dt = cb[1] + (cb[5] - cb[1]) * tt;
        } else {
            // Reference edge (0..11) from the case's test table. `at` stays 0.
            at = 0.0;
            let edge: i8 = match case {
                7 => t::TEST6[cfg][2],
                8 => t::TEST7[cfg][4],
                13 => t::TEST12[cfg][3],
                14 => t::TILING13_5_1[cfg][(subcfg - 1) as usize][0] - 1,
                _ => -1,
            };
            match edge {
                0 => {
                    let tt = cb[0] / (cb[0] - cb[1]);
                    bt = cb[3] + (cb[2] - cb[3]) * tt;
                    ct = cb[7] + (cb[6] - cb[7]) * tt;
                    dt = cb[4] + (cb[5] - cb[4]) * tt;
                }
                1 => {
                    let tt = cb[1] / (cb[1] - cb[2]);
                    bt = cb[0] + (cb[3] - cb[0]) * tt;
                    ct = cb[4] + (cb[7] - cb[4]) * tt;
                    dt = cb[5] + (cb[6] - cb[5]) * tt;
                }
                2 => {
                    let tt = cb[2] / (cb[2] - cb[3]);
                    bt = cb[1] + (cb[0] - cb[1]) * tt;
                    ct = cb[5] + (cb[4] - cb[5]) * tt;
                    dt = cb[6] + (cb[7] - cb[6]) * tt;
                }
                3 => {
                    let tt = cb[3] / (cb[3] - cb[0]);
                    bt = cb[2] + (cb[1] - cb[2]) * tt;
                    ct = cb[6] + (cb[5] - cb[6]) * tt;
                    dt = cb[7] + (cb[4] - cb[7]) * tt;
                }
                4 => {
                    let tt = cb[4] / (cb[4] - cb[5]);
                    bt = cb[7] + (cb[6] - cb[7]) * tt;
                    ct = cb[3] + (cb[2] - cb[3]) * tt;
                    dt = cb[0] + (cb[1] - cb[0]) * tt;
                }
                5 => {
                    let tt = cb[5] / (cb[5] - cb[6]);
                    bt = cb[4] + (cb[7] - cb[4]) * tt;
                    ct = cb[0] + (cb[3] - cb[0]) * tt;
                    dt = cb[1] + (cb[2] - cb[1]) * tt;
                }
                6 => {
                    let tt = cb[6] / (cb[6] - cb[7]);
                    bt = cb[5] + (cb[4] - cb[5]) * tt;
                    ct = cb[1] + (cb[0] - cb[1]) * tt;
                    dt = cb[2] + (cb[3] - cb[2]) * tt;
                }
                7 => {
                    let tt = cb[7] / (cb[7] - cb[4]);
                    bt = cb[6] + (cb[5] - cb[6]) * tt;
                    ct = cb[2] + (cb[1] - cb[2]) * tt;
                    dt = cb[3] + (cb[0] - cb[3]) * tt;
                }
                8 => {
                    let tt = cb[0] / (cb[0] - cb[4]);
                    bt = cb[3] + (cb[7] - cb[3]) * tt;
                    ct = cb[2] + (cb[6] - cb[2]) * tt;
                    dt = cb[1] + (cb[5] - cb[1]) * tt;
                }
                9 => {
                    let tt = cb[1] / (cb[1] - cb[5]);
                    bt = cb[0] + (cb[4] - cb[0]) * tt;
                    ct = cb[3] + (cb[7] - cb[3]) * tt;
                    dt = cb[2] + (cb[6] - cb[2]) * tt;
                }
                10 => {
                    let tt = cb[2] / (cb[2] - cb[6]);
                    bt = cb[1] + (cb[5] - cb[1]) * tt;
                    ct = cb[0] + (cb[4] - cb[0]) * tt;
                    dt = cb[3] + (cb[7] - cb[3]) * tt;
                }
                11 => {
                    let tt = cb[3] / (cb[3] - cb[7]);
                    bt = cb[2] + (cb[6] - cb[2]) * tt;
                    ct = cb[1] + (cb[5] - cb[1]) * tt;
                    dt = cb[0] + (cb[4] - cb[0]) * tt;
                }
                _ => return s < 0,
            }
        }

        let mut test = 0i32;
        if at >= 0.0 {
            test += 1;
        }
        if bt >= 0.0 {
            test += 2;
        }
        if ct >= 0.0 {
            test += 4;
        }
        if dt >= 0.0 {
            test += 8;
        }
        match test {
            0 | 1 | 2 | 3 | 4 | 6 | 8 | 9 | 12 => s > 0,
            5 => {
                if at * ct - bt * dt < EPS {
                    s > 0
                } else {
                    s < 0
                }
            }
            10 => {
                if at * ct - bt * dt >= EPS {
                    s > 0
                } else {
                    s < 0
                }
            }
            _ => s < 0,
        }
    }
}

/// Port of Lewiner's ambiguous-face test on the bilinear face interpolant.
fn test_face(cb: &[f32; 8], face: i8) -> bool {
    let (a, b, c, d) = match face.abs() {
        1 => (cb[0], cb[4], cb[5], cb[1]),
        2 => (cb[1], cb[5], cb[6], cb[2]),
        3 => (cb[2], cb[6], cb[7], cb[3]),
        4 => (cb[3], cb[7], cb[4], cb[0]),
        5 => (cb[0], cb[3], cb[2], cb[1]),
        6 => (cb[4], cb[7], cb[6], cb[5]),
        _ => return false,
    };
    let acbd = a * c - b * d;
    if acbd.abs() < EPS {
        face >= 0
    } else {
        (face as f32) * a * acbd >= 0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bvh::BoundedCsg;
    use crate::csg::{CsgNode, DecodedFeature, PrimitiveModifiers};
    use crate::mesh_validate::validate_topology;
    use fielddraft_protocol::{BoolOp, FeatureId, Primitive, Transform};

    fn feat(id: u32, op: BoolOp, prim: Primitive) -> DecodedFeature {
        DecodedFeature::from_parts(
            FeatureId::from_u32(id),
            format!("f{id}"),
            op,
            prim,
            Transform::IDENTITY,
            PrimitiveModifiers::NONE,
            vec![],
        )
    }

    fn bounded(features: &[DecodedFeature]) -> BoundedCsg {
        BoundedCsg::from_csg(&CsgNode::from_decoded(features))
    }

    #[test]
    fn mc33_sphere_is_watertight() {
        let b = bounded(&[feat(
            1,
            BoolOp::New,
            Primitive::Sphere {
                center: [0.0, 0.0, 0.0],
                radius: 10.0,
            },
        )]);
        let mesh = marching_cubes33(&b, 24);
        assert!(mesh.triangle_count() > 100, "sphere should mesh");
        let v = validate_topology(&mesh);
        // Watertight-by-construction: no cracks, no non-manifold edges.
        assert_eq!(v.boundary_edges, 0, "no open edges");
        assert_eq!(v.non_manifold_edges, 0, "manifold");
        assert_eq!(v.components, 1, "single shell");
    }

    #[test]
    fn mc33_box_minus_sphere_is_watertight() {
        // A CSG carve exercises ambiguous cases along the intersection.
        let b = bounded(&[
            feat(
                1,
                BoolOp::New,
                Primitive::Box {
                    center: [0.0, 0.0, 0.0],
                    half_extents: [10.0, 10.0, 10.0],
                },
            ),
            feat(
                2,
                BoolOp::Remove,
                Primitive::Sphere {
                    center: [10.0, 10.0, 10.0],
                    radius: 8.0,
                },
            ),
        ]);
        let mesh = marching_cubes33(&b, 28);
        assert!(mesh.triangle_count() > 100);
        let v = validate_topology(&mesh);
        assert_eq!(v.boundary_edges, 0, "no open edges on CSG carve");
        assert_eq!(v.non_manifold_edges, 0, "manifold on CSG carve");
    }

    #[test]
    fn emc_box_stays_watertight_and_adds_feature_verts() {
        // A pure box is all sharp edges → EMC should tent feature cells while
        // keeping the mesh closed (patch-boundary edges stay shared).
        let feats = [feat(
            1,
            BoolOp::New,
            Primitive::Box {
                center: [0.0, 0.0, 0.0],
                half_extents: [10.0, 10.0, 10.0],
            },
        )];
        let plain = marching_cubes33(&bounded(&feats), 20);
        let emc = extended_marching_cubes(&bounded(&feats), 20);
        assert!(emc.triangle_count() >= plain.triangle_count(), "EMC tents feature cells");
        let v = validate_topology(&emc);
        assert_eq!(v.boundary_edges, 0, "EMC keeps the mesh closed");
    }

    #[test]
    fn empty_scene_empty_mesh() {
        assert!(marching_cubes33(&BoundedCsg::Empty, 16).is_empty());
        assert!(extended_marching_cubes(&BoundedCsg::Empty, 16).is_empty());
    }
}
