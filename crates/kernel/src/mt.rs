//! Marching Tetrahedra mesher.
//!
//! A third surface-extraction option next to Manifold Dual Contouring
//! ([`crate::mdc`]) and Reach For The Arcs ([`crate::rfta`]). The grid is
//! the same uniform, AABB-expanded voxel lattice the DC path uses, but
//! each cube is split into six tetrahedra sharing the `0–6` body diagonal
//! and the zero level set is extracted per tetrahedron.
//!
//! Trade-offs (see `docs/SDFRaster.md` discussion): MT guarantees a
//! watertight, 2-manifold triangle soup with trivially consistent
//! topology and no QEF solve, but — unlike Dual Contouring — it cannot
//! place a vertex *on* a sharp CAD edge, so creases get rounded at the
//! grid scale. It is offered as a fast, robust baseline, not as the
//! quality default.
//!
//! The decomposition is crack-free because every cube uses the identical
//! corner-offset numbering, so the face diagonal a cube draws on a shared
//! face matches the one its neighbour draws on the same face.

use crate::bvh::BoundedCsg;
use crate::csg::{EvalMode, ProxyElement, MAX_CONTRIBUTORS};
use crate::dc::{sdf_normal, Mesh, CONTRIBUTOR_SENTINEL};
use std::collections::HashMap;

/// Mesh a CSG tree by Marching Tetrahedra on a uniform grid.
///
/// `resolution` is the number of cells along the *longest* AABB axis,
/// matching [`crate::dc::dual_contour_with_primitives`] so the resolution
/// slider behaves identically across meshers. An empty scene (or
/// `resolution == 0`) yields an empty mesh.
pub fn marching_tetrahedra(node: &BoundedCsg, resolution: u32) -> Mesh {
    if resolution == 0 {
        return Mesh::default();
    }
    let raw = node.aabb();
    if raw.is_empty() {
        return Mesh::default();
    }
    let raw_size = raw.size();
    let max_dim = raw_size[0].max(raw_size[1]).max(raw_size[2]);
    if max_dim <= 0.0 {
        return Mesh::default();
    }

    let cell = max_dim / resolution as f32;
    // Same margin as the DC path so every surface vertex sits inside the
    // sampled lattice and edges always have a sign on both ends.
    let grid = raw.expanded(cell * crate::dc::DC_MARGIN_CELLS);
    let size = grid.size();
    let min = grid.min;

    let nx = ((size[0] / cell).ceil() as usize).max(1);
    let ny = ((size[1] / cell).ceil() as usize).max(1);
    let nz = ((size[2] / cell).ceil() as usize).max(1);

    let normal_eps = cell * 0.25;
    let contributor_eps = cell * 0.5;

    // ── Corner sample grid ──────────────────────────────────────────
    let (gx, gy, gz) = (nx + 1, ny + 1, nz + 1);
    let corner_pos = |i: usize, j: usize, k: usize| -> [f32; 3] {
        [
            min[0] + i as f32 * cell,
            min[1] + j as f32 * cell,
            min[2] + k as f32 * cell,
        ]
    };
    let gidx = |i: usize, j: usize, k: usize| -> usize { i + gx * (j + gy * k) };

    let mut values = vec![0f32; gx * gy * gz];
    for k in 0..gz {
        for j in 0..gy {
            for i in 0..gx {
                values[gidx(i, j, k)] = node.sdf(corner_pos(i, j, k), EvalMode::Visual);
            }
        }
    }

    // Cube corner offsets (i,j,k) with the canonical numbering used by
    // the 6-tet decomposition below.
    const CUBE: [[usize; 3]; 8] = [
        [0, 0, 0], // 0
        [1, 0, 0], // 1
        [1, 1, 0], // 2
        [0, 1, 0], // 3
        [0, 0, 1], // 4
        [1, 0, 1], // 5
        [1, 1, 1], // 6
        [0, 1, 1], // 7
    ];
    // Six tetrahedra sharing the 0–6 body diagonal, walking the belt
    // 1→2→3→7→4→5→1. Identical for every cube ⇒ crack-free.
    const TETS: [[usize; 4]; 6] = [
        [0, 6, 1, 2],
        [0, 6, 2, 3],
        [0, 6, 3, 7],
        [0, 6, 7, 4],
        [0, 6, 4, 5],
        [0, 6, 5, 1],
    ];

    let mut ctx = TetCtx {
        min,
        cell,
        gx,
        gy,
        node,
        normal_eps,
        contributor_eps,
        mesh: Mesh::default(),
        // Weld intersection points by the unordered global-corner edge
        // they lie on, so the output is an indexed, watertight mesh.
        edge_cache: HashMap::new(),
    };

    for ck in 0..nz {
        for cj in 0..ny {
            for ci in 0..nx {
                for tet in &TETS {
                    // Gather the four tet corners in global coords.
                    let mut g = [(0usize, 0usize, 0usize); 4];
                    let mut val = [0f32; 4];
                    for t in 0..4 {
                        let c = tet[t];
                        let (i, j, k) =
                            (ci + CUBE[c][0], cj + CUBE[c][1], ck + CUBE[c][2]);
                        g[t] = (i, j, k);
                        val[t] = values[gidx(i, j, k)];
                    }
                    ctx.emit_tet(&g, &val);
                }
            }
        }
    }

    ctx.mesh
}

/// Per-mesh state threaded through tetrahedron emission. Bundled into a
/// struct so the interpolation / triangle helpers can borrow `mesh` and
/// `edge_cache` together without the closure-in-closure borrow tangle.
struct TetCtx<'a> {
    min: [f32; 3],
    cell: f32,
    gx: usize,
    gy: usize,
    node: &'a BoundedCsg,
    normal_eps: f32,
    contributor_eps: f32,
    mesh: Mesh,
    edge_cache: HashMap<u64, u32>,
}

impl TetCtx<'_> {
    /// Emit the triangles for a single tetrahedron.
    fn emit_tet(&mut self, g: &[(usize, usize, usize); 4], val: &[f32; 4]) {
        // Inside = strictly negative SDF. Treat an exact zero as outside
        // so a vertex on the surface never spawns a degenerate triangle.
        let mut code = 0u8;
        if val[0] < 0.0 {
            code |= 1;
        }
        if val[1] < 0.0 {
            code |= 2;
        }
        if val[2] < 0.0 {
            code |= 4;
        }
        if val[3] < 0.0 {
            code |= 8;
        }
        if code == 0x00 || code == 0x0F {
            return;
        }

        // Bourke's tetrahedron polygonisation. Each case lists the
        // crossing edges as (corner,corner) pairs; orientation is fixed
        // afterwards against the SDF gradient so windings stay outward
        // across the 6-tet decomposition regardless of the table's local
        // convention.
        let tris: &[[(usize, usize); 3]] = match code {
            0x01 | 0x0E => &[[(0, 1), (0, 2), (0, 3)]],
            0x02 | 0x0D => &[[(1, 0), (1, 3), (1, 2)]],
            0x04 | 0x0B => &[[(2, 0), (2, 1), (2, 3)]],
            0x08 | 0x07 => &[[(3, 0), (3, 2), (3, 1)]],
            // verts 0,1 on one side
            0x03 | 0x0C => &[[(0, 3), (0, 2), (1, 3)], [(1, 3), (0, 2), (1, 2)]],
            // verts 0,2 on one side
            0x05 | 0x0A => &[[(0, 1), (2, 3), (0, 3)], [(0, 1), (1, 2), (2, 3)]],
            // verts 1,2 on one side (crossing edges 01,02,13,23)
            0x06 | 0x09 => &[[(0, 1), (0, 2), (2, 3)], [(0, 1), (2, 3), (1, 3)]],
            _ => &[],
        };

        for tri in tris {
            let ia = self.vert(g, val, tri[0].0, tri[0].1);
            let ib = self.vert(g, val, tri[1].0, tri[1].1);
            let ic = self.vert(g, val, tri[2].0, tri[2].1);
            if ia == ib || ib == ic || ia == ic {
                continue; // degenerate
            }
            self.orient_and_push(ia, ib, ic);
        }
    }

    /// Global linear corner index.
    fn gidx(&self, i: usize, j: usize, k: usize) -> usize {
        i + self.gx * (j + self.gy * k)
    }

    fn corner_pos(&self, i: usize, j: usize, k: usize) -> [f32; 3] {
        [
            self.min[0] + i as f32 * self.cell,
            self.min[1] + j as f32 * self.cell,
            self.min[2] + k as f32 * self.cell,
        ]
    }

    /// Interpolated intersection vertex on edge (a,b), deduped by the
    /// global edge key so shared edges weld into one vertex.
    fn vert(&mut self, g: &[(usize, usize, usize); 4], val: &[f32; 4], a: usize, b: usize) -> u32 {
        let ka = self.gidx(g[a].0, g[a].1, g[a].2) as u64;
        let kb = self.gidx(g[b].0, g[b].1, g[b].2) as u64;
        let key = if ka < kb { (ka << 32) | kb } else { (kb << 32) | ka };
        if let Some(&idx) = self.edge_cache.get(&key) {
            return idx;
        }
        let pa = self.corner_pos(g[a].0, g[a].1, g[a].2);
        let pb = self.corner_pos(g[b].0, g[b].1, g[b].2);
        let (sa, sb) = (val[a], val[b]);
        let denom = sa - sb;
        let t = if denom.abs() > 1e-12 {
            (sa / denom).clamp(0.0, 1.0)
        } else {
            0.5
        };
        let pos = [
            pa[0] + t * (pb[0] - pa[0]),
            pa[1] + t * (pb[1] - pa[1]),
            pa[2] + t * (pb[2] - pa[2]),
        ];
        let normal = sdf_normal(self.node, pos, self.normal_eps);

        let (_, cs) = self.node.evaluate(pos, self.contributor_eps, EvalMode::Visual);
        let mut element = cs.element();
        if element == ProxyElement::None {
            element = ProxyElement::Face;
        }
        let slice = cs.as_slice();
        let idx = (self.mesh.positions.len() / 3) as u32;
        self.mesh.positions.extend_from_slice(&pos);
        self.mesh.normals.extend_from_slice(&normal);
        self.mesh.classifications.push(element.tag());
        for slot in 0..MAX_CONTRIBUTORS {
            let c = slice
                .get(slot)
                .map(|id| id.feature_id())
                .unwrap_or(CONTRIBUTOR_SENTINEL);
            self.mesh.contributors.push(c);
        }
        self.edge_cache.insert(key, idx);
        idx
    }

    /// Append a triangle, flipping its winding so the geometric face
    /// normal agrees with the SDF gradient at the centroid (outward).
    fn orient_and_push(&mut self, ia: u32, ib: u32, ic: u32) {
        let p = |m: &Mesh, i: u32| {
            let b = i as usize * 3;
            [m.positions[b], m.positions[b + 1], m.positions[b + 2]]
        };
        let (a, b, c) = (p(&self.mesh, ia), p(&self.mesh, ib), p(&self.mesh, ic));
        let e1 = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let e2 = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let fnorm = [
            e1[1] * e2[2] - e1[2] * e2[1],
            e1[2] * e2[0] - e1[0] * e2[2],
            e1[0] * e2[1] - e1[1] * e2[0],
        ];
        let centroid = [
            (a[0] + b[0] + c[0]) / 3.0,
            (a[1] + b[1] + c[1]) / 3.0,
            (a[2] + b[2] + c[2]) / 3.0,
        ];
        let grad = sdf_normal(self.node, centroid, self.normal_eps);
        let dot = fnorm[0] * grad[0] + fnorm[1] * grad[1] + fnorm[2] * grad[2];
        if dot >= 0.0 {
            self.mesh.indices.extend_from_slice(&[ia, ib, ic]);
        } else {
            self.mesh.indices.extend_from_slice(&[ia, ic, ib]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::body::build_bounded_for_body;
    use crate::csg::{DecodedFeature, PrimitiveModifiers};
    use fielddraft_protocol::{BoolOp, FeatureId, Primitive, Transform};

    fn sphere(id: u32, center: [f32; 3], radius: f32) -> DecodedFeature {
        DecodedFeature::from_parts(
            FeatureId::from_u32(id),
            format!("Sphere {id}"),
            BoolOp::New,
            Primitive::Sphere { center, radius },
            Transform::IDENTITY,
            PrimitiveModifiers::NONE,
            vec![],
        )
    }

    #[test]
    fn sphere_is_watertight_and_outward() {
        let feats = vec![sphere(1, [0.0, 0.0, 0.0], 1.0)];
        let node = build_bounded_for_body(&feats, &[0]);
        let mesh = marching_tetrahedra(&node, 24);

        assert!(!mesh.is_empty(), "sphere should produce geometry");
        assert_eq!(mesh.positions.len(), mesh.normals.len());
        assert_eq!(
            mesh.contributors.len(),
            mesh.vertex_count() * MAX_CONTRIBUTORS
        );

        // Watertight closed surface ⇒ every edge shared by exactly two
        // triangles (each undirected edge appears an even number of times).
        let mut edge_count: HashMap<(u32, u32), u32> = HashMap::new();
        for tri in mesh.indices.chunks_exact(3) {
            for &(a, b) in &[(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])] {
                let key = if a < b { (a, b) } else { (b, a) };
                *edge_count.entry(key).or_insert(0) += 1;
            }
        }
        for (_, &c) in edge_count.iter() {
            assert_eq!(c, 2, "non-manifold / open edge found");
        }

        // Vertices sit on the unit sphere within a grid-scale tolerance,
        // and stored normals point radially outward.
        for v in 0..mesh.vertex_count() {
            let p = [
                mesh.positions[v * 3],
                mesh.positions[v * 3 + 1],
                mesh.positions[v * 3 + 2],
            ];
            let r = (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt();
            assert!((r - 1.0).abs() < 0.15, "vertex off the sphere: r={r}");
            let n = [
                mesh.normals[v * 3],
                mesh.normals[v * 3 + 1],
                mesh.normals[v * 3 + 2],
            ];
            let dot = (p[0] * n[0] + p[1] * n[1] + p[2] * n[2]) / r.max(1e-6);
            assert!(dot > 0.5, "normal not outward: dot={dot}");
        }
    }

    #[test]
    fn empty_resolution_is_empty() {
        let feats = vec![sphere(1, [0.0, 0.0, 0.0], 1.0)];
        let node = build_bounded_for_body(&feats, &[0]);
        assert!(marching_tetrahedra(&node, 0).is_empty());
    }
}
