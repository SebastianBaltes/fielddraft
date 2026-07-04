//! Dual Marching Cubes (Nielson 2004) — a quad-dominant mesher dual to
//! Marching Cubes.
//!
//! Ported from `dominikwodniok/dualmc` (BSD-3). Each MC cube contributes up to
//! four *dual points* (one per MC surface patch), placed at the mean of that
//! patch's edge crossings. For every sign-changing primal grid edge the four
//! cells around it contribute one dual point each, joined into a quad. The
//! result has the same separating surface as MC but is built from
//! well-shaped quads instead of the sliver triangles MC often emits.
//!
//! The manifold variant (Wenger, *Isosurfaces* §3.3.5) is used: when a
//! problematic C16/C19 configuration shares its ambiguous face with another
//! such cell, the cube code is inverted before dual-point lookup, guaranteeing
//! a manifold mesh (at the cost of exact MC duality). Quads are split into two
//! triangles for the engine's triangle-only `Mesh`; winding is left to the
//! export pass's `fix_orientation`.

use crate::bvh::BoundedCsg;
use crate::csg::EvalMode;
use crate::dc::{sdf_normal, Mesh, DC_MARGIN_CELLS};
use crate::dual_marching_cubes_tables::{DUAL_POINTS, PROBLEMATIC_CONFIGS};
use std::collections::HashMap;

// Edge bit flags (EDGEk = 1<<k), matching the generated `DUAL_POINTS` masks.
const EDGE0: u16 = 1 << 0;
const EDGE1: u16 = 1 << 1;
const EDGE2: u16 = 1 << 2;
const EDGE3: u16 = 1 << 3;
const EDGE4: u16 = 1 << 4;
const EDGE5: u16 = 1 << 5;
const EDGE6: u16 = 1 << 6;
const EDGE7: u16 = 1 << 7;
const EDGE8: u16 = 1 << 8;
const EDGE9: u16 = 1 << 9;
const EDGE10: u16 = 1 << 10;
const EDGE11: u16 = 1 << 11;

/// Dual Marching Cubes at `resolution` cells along the longest AABB axis.
pub fn dual_marching_cubes(node: &BoundedCsg, resolution: u32) -> Mesh {
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
    if sx < 3 || sy < 3 || sz < 3 {
        return Mesh::default();
    }

    let mut d = Dmc {
        sx,
        sy,
        sz,
        cell,
        gmin,
        values: vec![0.0; sx * sy * sz],
        positions: Vec::new(),
        point_to_index: HashMap::new(),
    };
    for k in 0..sz {
        for j in 0..sy {
            for i in 0..sx {
                let p = d.corner_pos(i, j, k);
                d.values[(k * sy + j) * sx + i] = node.sdf(p, EvalMode::Visual);
            }
        }
    }

    let indices = d.build();
    if indices.is_empty() {
        return Mesh::default();
    }

    let vcount = d.positions.len() / 3;
    let mut normals = vec![0.0f32; d.positions.len()];
    for v in 0..vcount {
        let p = [
            d.positions[v * 3],
            d.positions[v * 3 + 1],
            d.positions[v * 3 + 2],
        ];
        let n = sdf_normal(node, p, cell * 0.25);
        normals[v * 3] = n[0];
        normals[v * 3 + 1] = n[1];
        normals[v * 3 + 2] = n[2];
    }

    Mesh {
        positions: d.positions,
        normals,
        indices,
        ..Mesh::default()
    }
}

struct Dmc {
    sx: usize,
    sy: usize,
    sz: usize,
    cell: f32,
    gmin: [f32; 3],
    values: Vec<f32>,
    positions: Vec<f32>,
    /// (linear cell id, dual-point code) → vertex id, for shared vertices.
    point_to_index: HashMap<(u32, u16), u32>,
}

/// Crossing fraction of `iso` between endpoint values `a`,`b` (guarded).
#[inline]
fn frac(a: f32, b: f32) -> f32 {
    let denom = b - a;
    if denom.abs() < 1e-20 {
        0.5
    } else {
        (0.0 - a) / denom
    }
}

impl Dmc {
    #[inline]
    fn corner_pos(&self, i: usize, j: usize, k: usize) -> [f32; 3] {
        [
            self.gmin[0] + i as f32 * self.cell,
            self.gmin[1] + j as f32 * self.cell,
            self.gmin[2] + k as f32 * self.cell,
        ]
    }

    #[inline]
    fn v(&self, i: usize, j: usize, k: usize) -> f32 {
        self.values[(k * self.sy + j) * self.sx + i]
    }

    #[inline]
    fn cell_id(&self, i: usize, j: usize, k: usize) -> u32 {
        ((k * self.sy + j) * self.sx + i) as u32
    }

    /// 8-corner sign code of the cube at (cx,cy,cz). Inside = sdf >= 0.
    fn cell_code(&self, cx: usize, cy: usize, cz: usize) -> u8 {
        let mut code = 0u8;
        if self.v(cx, cy, cz) >= 0.0 {
            code |= 1;
        }
        if self.v(cx + 1, cy, cz) >= 0.0 {
            code |= 2;
        }
        if self.v(cx, cy + 1, cz) >= 0.0 {
            code |= 4;
        }
        if self.v(cx + 1, cy + 1, cz) >= 0.0 {
            code |= 8;
        }
        if self.v(cx, cy, cz + 1) >= 0.0 {
            code |= 16;
        }
        if self.v(cx + 1, cy, cz + 1) >= 0.0 {
            code |= 32;
        }
        if self.v(cx, cy + 1, cz + 1) >= 0.0 {
            code |= 64;
        }
        if self.v(cx + 1, cy + 1, cz + 1) >= 0.0 {
            code |= 128;
        }
        code
    }

    /// The dual-point code (edge mask) for the patch of cube (cx,cy,cz) that
    /// touches `edge`, applying the manifold cube-code inversion.
    fn dual_point_code(&self, cx: usize, cy: usize, cz: usize, edge: u16) -> u16 {
        let mut cube_code = self.cell_code(cx, cy, cz) as usize;
        // Manifold fix (Wenger §3.3.5): invert a problematic C16/C19 cube when
        // its ambiguous face is shared with another such cube.
        let direction = PROBLEMATIC_CONFIGS[cube_code];
        if direction != 255 {
            let mut nc = [cx as i32, cy as i32, cz as i32];
            let component = (direction >> 1) as usize;
            let delta = if direction & 1 == 1 { 1 } else { -1 };
            nc[component] += delta;
            let dim = [self.sx as i32, self.sy as i32, self.sz as i32][component];
            if nc[component] >= 0 && nc[component] < dim - 1 {
                let ncode = self.cell_code(nc[0] as usize, nc[1] as usize, nc[2] as usize) as usize;
                if PROBLEMATIC_CONFIGS[ncode] != 255 {
                    cube_code ^= 0xff;
                }
            }
        }
        for &pc in &DUAL_POINTS[cube_code] {
            if pc & edge != 0 {
                return pc;
            }
        }
        0
    }

    /// Dual point (world position) of a cube patch from its edge-mask code.
    fn dual_point(&self, cx: usize, cy: usize, cz: usize, code: u16) -> [f32; 3] {
        let mut p = [0.0f32; 3];
        let mut n = 0.0f32;
        macro_rules! acc {
            ($px:expr, $py:expr, $pz:expr) => {{
                p[0] += $px;
                p[1] += $py;
                p[2] += $pz;
                n += 1.0;
            }};
        }
        if code & EDGE0 != 0 {
            acc!(frac(self.v(cx, cy, cz), self.v(cx + 1, cy, cz)), 0.0, 0.0);
        }
        if code & EDGE1 != 0 {
            acc!(1.0, 0.0, frac(self.v(cx + 1, cy, cz), self.v(cx + 1, cy, cz + 1)));
        }
        if code & EDGE2 != 0 {
            acc!(frac(self.v(cx, cy, cz + 1), self.v(cx + 1, cy, cz + 1)), 0.0, 1.0);
        }
        if code & EDGE3 != 0 {
            acc!(0.0, 0.0, frac(self.v(cx, cy, cz), self.v(cx, cy, cz + 1)));
        }
        if code & EDGE4 != 0 {
            acc!(frac(self.v(cx, cy + 1, cz), self.v(cx + 1, cy + 1, cz)), 1.0, 0.0);
        }
        if code & EDGE5 != 0 {
            acc!(1.0, 1.0, frac(self.v(cx + 1, cy + 1, cz), self.v(cx + 1, cy + 1, cz + 1)));
        }
        if code & EDGE6 != 0 {
            acc!(frac(self.v(cx, cy + 1, cz + 1), self.v(cx + 1, cy + 1, cz + 1)), 1.0, 1.0);
        }
        if code & EDGE7 != 0 {
            acc!(0.0, 1.0, frac(self.v(cx, cy + 1, cz), self.v(cx, cy + 1, cz + 1)));
        }
        if code & EDGE8 != 0 {
            acc!(0.0, frac(self.v(cx, cy, cz), self.v(cx, cy + 1, cz)), 0.0);
        }
        if code & EDGE9 != 0 {
            acc!(1.0, frac(self.v(cx + 1, cy, cz), self.v(cx + 1, cy + 1, cz)), 0.0);
        }
        if code & EDGE10 != 0 {
            acc!(1.0, frac(self.v(cx + 1, cy, cz + 1), self.v(cx + 1, cy + 1, cz + 1)), 1.0);
        }
        if code & EDGE11 != 0 {
            acc!(0.0, frac(self.v(cx, cy, cz + 1), self.v(cx, cy + 1, cz + 1)), 1.0);
        }
        let inv = if n > 0.0 { 1.0 / n } else { 0.0 };
        // grid-space (cx + local) → world.
        [
            self.gmin[0] + (cx as f32 + p[0] * inv) * self.cell,
            self.gmin[1] + (cy as f32 + p[1] * inv) * self.cell,
            self.gmin[2] + (cz as f32 + p[2] * inv) * self.cell,
        ]
    }

    /// Shared dual-point vertex id for cube (cx,cy,cz)'s patch touching `edge`.
    fn shared_index(&mut self, cx: usize, cy: usize, cz: usize, edge: u16) -> u32 {
        let code = self.dual_point_code(cx, cy, cz, edge);
        let key = (self.cell_id(cx, cy, cz), code);
        if let Some(&id) = self.point_to_index.get(&key) {
            return id;
        }
        let pos = self.dual_point(cx, cy, cz, code);
        let id = (self.positions.len() / 3) as u32;
        self.positions.extend_from_slice(&pos);
        self.point_to_index.insert(key, id);
        id
    }

    /// Build the quad mesh (as a triangle index list) with shared vertices.
    fn build(&mut self) -> Vec<u32> {
        let mut indices: Vec<u32> = Vec::new();
        let quad = |i0: u32, i1: u32, i2: u32, i3: u32, out: &mut Vec<u32>| {
            out.extend_from_slice(&[i0, i1, i2]);
            out.extend_from_slice(&[i0, i2, i3]);
        };
        let (rx, ry, rz) = (self.sx - 2, self.sy - 2, self.sz - 2);
        for z in 0..rz {
            for y in 0..ry {
                for x in 0..rx {
                    // Quad around the intersected x-edge.
                    if z > 0 && y > 0 {
                        let a = self.v(x, y, z);
                        let b = self.v(x + 1, y, z);
                        let entering = a < 0.0 && b >= 0.0;
                        let exiting = a >= 0.0 && b < 0.0;
                        if entering || exiting {
                            let i0 = self.shared_index(x, y, z, EDGE0);
                            let i1 = self.shared_index(x, y, z - 1, EDGE2);
                            let i2 = self.shared_index(x, y - 1, z - 1, EDGE6);
                            let i3 = self.shared_index(x, y - 1, z, EDGE4);
                            if entering {
                                quad(i0, i1, i2, i3, &mut indices);
                            } else {
                                quad(i0, i3, i2, i1, &mut indices);
                            }
                        }
                    }
                    // Quad around the intersected y-edge.
                    if z > 0 && x > 0 {
                        let a = self.v(x, y, z);
                        let b = self.v(x, y + 1, z);
                        let entering = a < 0.0 && b >= 0.0;
                        let exiting = a >= 0.0 && b < 0.0;
                        if entering || exiting {
                            let i0 = self.shared_index(x, y, z, EDGE8);
                            let i1 = self.shared_index(x, y, z - 1, EDGE11);
                            let i2 = self.shared_index(x - 1, y, z - 1, EDGE10);
                            let i3 = self.shared_index(x - 1, y, z, EDGE9);
                            if exiting {
                                quad(i0, i1, i2, i3, &mut indices);
                            } else {
                                quad(i0, i3, i2, i1, &mut indices);
                            }
                        }
                    }
                    // Quad around the intersected z-edge.
                    if x > 0 && y > 0 {
                        let a = self.v(x, y, z);
                        let b = self.v(x, y, z + 1);
                        let entering = a < 0.0 && b >= 0.0;
                        let exiting = a >= 0.0 && b < 0.0;
                        if entering || exiting {
                            let i0 = self.shared_index(x, y, z, EDGE3);
                            let i1 = self.shared_index(x - 1, y, z, EDGE1);
                            let i2 = self.shared_index(x - 1, y - 1, z, EDGE5);
                            let i3 = self.shared_index(x, y - 1, z, EDGE7);
                            if exiting {
                                quad(i0, i1, i2, i3, &mut indices);
                            } else {
                                quad(i0, i3, i2, i1, &mut indices);
                            }
                        }
                    }
                }
            }
        }
        // Drop any quad-diagonal degenerate triangles.
        let mut clean = Vec::with_capacity(indices.len());
        for t in indices.chunks_exact(3) {
            if t[0] != t[1] && t[1] != t[2] && t[0] != t[2] {
                clean.extend_from_slice(t);
            }
        }
        clean
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
    fn dmc_sphere_is_manifold_and_closed() {
        let b = bounded(&[feat(
            1,
            BoolOp::New,
            Primitive::Sphere {
                center: [0.0, 0.0, 0.0],
                radius: 10.0,
            },
        )]);
        let mesh = dual_marching_cubes(&b, 24);
        assert!(mesh.triangle_count() > 100, "sphere should mesh");
        let v = validate_topology(&mesh);
        assert_eq!(v.boundary_edges, 0, "manifold DMC has no open edges");
        assert_eq!(v.non_manifold_edges, 0, "manifold variant");
        assert_eq!(v.components, 1, "single shell");
    }

    #[test]
    fn dmc_box_minus_sphere_is_closed() {
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
        let mesh = dual_marching_cubes(&b, 28);
        assert!(mesh.triangle_count() > 100);
        let v = validate_topology(&mesh);
        assert_eq!(v.boundary_edges, 0, "no open edges on CSG carve");
        assert_eq!(v.non_manifold_edges, 0, "manifold on CSG carve");
    }

    #[test]
    fn empty_scene_empty_mesh() {
        assert!(dual_marching_cubes(&BoundedCsg::Empty, 16).is_empty());
    }
}
