//! K13.0: Mesh-to-SDF — evaluate the signed distance from a
//! point to an imported triangle mesh. Uses a BVH (bounding
//! volume hierarchy) for O(log N) nearest-triangle queries,
//! and angle-weighted pseudo-normals for sign determination.
//!
//! This module is intentionally self-contained: no dependency
//! on the rest of the kernel's CSG/BVH infrastructure (those
//! operate on the SDF level, not on triangles). The only
//! public type is [`MeshSdf`], which the feature system wraps
//! in a `Primitive::ImportedMesh` leaf.

use serde::{Deserialize, Serialize};

/// K13.3: result of a manifold/watertightness check.
#[derive(Debug, Clone, Serialize)]
pub struct ManifoldReport {
    pub triangle_count: u32,
    pub open_edges: u32,
    pub non_manifold_edges: u32,
    pub is_watertight: bool,
}

// ─── BVH internals ───────────────────────────────────────────

const BVH_LEAF_SIZE: usize = 4;

#[derive(Debug, Clone, PartialEq)]
struct BvhNode {
    aabb_min: [f32; 3],
    aabb_max: [f32; 3],
    /// For interior: left child index (right = left + 1).
    /// For leaf: first triangle index in `tri_order`.
    offset: u32,
    /// 0 for interior nodes, >0 = triangle count for leaves.
    count: u32,
}

/// Fast Winding Number data per BVH node (Barill et al. 2018, order-0 dipole).
#[derive(Debug, Clone, PartialEq)]
struct FwnNode {
    /// Area-weighted center of mass.
    center: [f64; 3],
    /// Sum of area-weighted normals (0.5 * cross(e1, e2) for each triangle).
    area_normal: [f64; 3],
    /// Max squared distance from center to any vertex in this subtree.
    radius_sq: f64,
}

/// Precomputed 3D SDF volume for GPU texture upload.
#[derive(Debug, Clone)]
pub struct BakedVolume {
    pub data: Vec<f32>,
    pub dims: [u32; 3],
    pub aabb_min: [f32; 3],
    pub aabb_max: [f32; 3],
}

impl BakedVolume {
    pub fn sample_trilinear(&self, p: [f32; 3]) -> f32 {
        let [nx, ny, nz] = self.dims;
        let uvw = [
            (p[0] - self.aabb_min[0]) / (self.aabb_max[0] - self.aabb_min[0]),
            (p[1] - self.aabb_min[1]) / (self.aabb_max[1] - self.aabb_min[1]),
            (p[2] - self.aabb_min[2]) / (self.aabb_max[2] - self.aabb_min[2]),
        ];
        let fx = uvw[0] * nx as f32 - 0.5;
        let fy = uvw[1] * ny as f32 - 0.5;
        let fz = uvw[2] * nz as f32 - 0.5;
        let x0 = (fx.floor() as i32).clamp(0, nx as i32 - 1) as usize;
        let y0 = (fy.floor() as i32).clamp(0, ny as i32 - 1) as usize;
        let z0 = (fz.floor() as i32).clamp(0, nz as i32 - 1) as usize;
        let x1 = (x0 + 1).min(nx as usize - 1);
        let y1 = (y0 + 1).min(ny as usize - 1);
        let z1 = (z0 + 1).min(nz as usize - 1);
        let tx = fx - fx.floor();
        let ty = fy - fy.floor();
        let tz = fz - fz.floor();
        let idx = |x: usize, y: usize, z: usize| z * (nx as usize * ny as usize) + y * nx as usize + x;
        let c000 = self.data[idx(x0, y0, z0)];
        let c100 = self.data[idx(x1, y0, z0)];
        let c010 = self.data[idx(x0, y1, z0)];
        let c110 = self.data[idx(x1, y1, z0)];
        let c001 = self.data[idx(x0, y0, z1)];
        let c101 = self.data[idx(x1, y0, z1)];
        let c011 = self.data[idx(x0, y1, z1)];
        let c111 = self.data[idx(x1, y1, z1)];
        let c00 = c000 * (1.0 - tx) + c100 * tx;
        let c10 = c010 * (1.0 - tx) + c110 * tx;
        let c01 = c001 * (1.0 - tx) + c101 * tx;
        let c11 = c011 * (1.0 - tx) + c111 * tx;
        let c0 = c00 * (1.0 - ty) + c10 * ty;
        let c1 = c01 * (1.0 - ty) + c11 * ty;
        c0 * (1.0 - tz) + c1 * tz
    }
}

/// A triangle mesh converted to a queryable signed distance
/// field. Immutable after construction — parameter edits
/// require rebuilding from the source triangles.
#[derive(Debug, Clone, PartialEq)]
pub struct MeshSdf {
    /// Flat array of triangle vertices: 9 floats per triangle
    /// (v0.x, v0.y, v0.z, v1.x, v1.y, v1.z, v2.x, v2.y, v2.z).
    tris: Vec<f32>,
    /// Per-triangle AABB: 6 floats per triangle (min.xyz, max.xyz).
    bboxes: Vec<f32>,
    /// Overall AABB of the entire mesh.
    pub aabb_min: [f32; 3],
    pub aabb_max: [f32; 3],
    /// BVH node array (flat binary tree).
    bvh_nodes: Vec<BvhNode>,
    /// Permuted triangle indices for BVH leaf traversal order.
    tri_order: Vec<u32>,
    /// True when face normals point inward (CW winding).
    sign_flip: bool,
    /// Fast Winding Number data per BVH node (Barill et al. 2018).
    fwn_nodes: Vec<FwnNode>,
}

// ─── Custom Serialize/Deserialize ────────────────────────────
// Only serialize tris + bboxes + aabb; rebuild BVH on load.

impl Serialize for MeshSdf {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut st = s.serialize_struct("MeshSdf", 4)?;
        st.serialize_field("tris", &self.tris)?;
        st.serialize_field("bboxes", &self.bboxes)?;
        st.serialize_field("aabb_min", &self.aabb_min)?;
        st.serialize_field("aabb_max", &self.aabb_max)?;
        st.end()
    }
}

impl<'de> Deserialize<'de> for MeshSdf {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Raw {
            tris: Vec<f32>,
            bboxes: Vec<f32>,
            aabb_min: [f32; 3],
            aabb_max: [f32; 3],
        }
        let raw = Raw::deserialize(d)?;
        let tri_count = raw.tris.len() / 9;
        let (bvh_nodes, tri_order) = build_bvh(&raw.tris, &raw.bboxes, tri_count);
        let sign_flip = compute_sign_flip(&raw.tris);
        let fwn_nodes = build_fwn_data(&raw.tris, &bvh_nodes, &tri_order);
        Ok(Self {
            tris: raw.tris,
            bboxes: raw.bboxes,
            aabb_min: raw.aabb_min,
            aabb_max: raw.aabb_max,
            bvh_nodes,
            tri_order,
            sign_flip,
            fwn_nodes,
        })
    }
}

impl MeshSdf {
    pub fn tris(&self) -> &[f32] {
        &self.tris
    }

    pub fn tri_count(&self) -> usize {
        self.tris.len() / 9
    }

    /// Build a flat display mesh from the stored triangles.
    /// Returns (positions, normals, indices) suitable for GPU rendering.
    pub fn to_display_mesh(&self) -> (Vec<f32>, Vec<f32>, Vec<u32>) {
        let tri_count = self.tri_count();
        if tri_count == 0 {
            return (vec![], vec![], vec![]);
        }

        use std::collections::HashMap;
        let quantize = |v: f32| -> i64 { (v * 10000.0).round() as i64 };

        let mut vert_map: HashMap<(i64, i64, i64), u32> = HashMap::new();
        let mut positions: Vec<f32> = Vec::with_capacity(tri_count * 3);
        let mut indices: Vec<u32> = Vec::with_capacity(tri_count * 3);

        for t in 0..tri_count {
            let base = t * 9;
            for v in 0..3 {
                let vb = base + v * 3;
                let key = (
                    quantize(self.tris[vb]),
                    quantize(self.tris[vb + 1]),
                    quantize(self.tris[vb + 2]),
                );
                let idx = *vert_map.entry(key).or_insert_with(|| {
                    let i = (positions.len() / 3) as u32;
                    positions.push(self.tris[vb]);
                    positions.push(self.tris[vb + 1]);
                    positions.push(self.tris[vb + 2]);
                    i
                });
                indices.push(idx);
            }
        }

        let vert_count = positions.len() / 3;
        let mut normals = vec![0.0f32; vert_count * 3];

        for t in 0..tri_count {
            let i0 = indices[t * 3] as usize;
            let i1 = indices[t * 3 + 1] as usize;
            let i2 = indices[t * 3 + 2] as usize;

            let v0 = [positions[i0*3], positions[i0*3+1], positions[i0*3+2]];
            let v1 = [positions[i1*3], positions[i1*3+1], positions[i1*3+2]];
            let v2 = [positions[i2*3], positions[i2*3+1], positions[i2*3+2]];

            let e1 = [v1[0]-v0[0], v1[1]-v0[1], v1[2]-v0[2]];
            let e2 = [v2[0]-v0[0], v2[1]-v0[1], v2[2]-v0[2]];
            let nx = e1[1]*e2[2] - e1[2]*e2[1];
            let ny = e1[2]*e2[0] - e1[0]*e2[2];
            let nz = e1[0]*e2[1] - e1[1]*e2[0];

            for &idx in &[i0, i1, i2] {
                normals[idx*3]   += nx;
                normals[idx*3+1] += ny;
                normals[idx*3+2] += nz;
            }
        }

        for v in 0..vert_count {
            let b = v * 3;
            let len = (normals[b]*normals[b] + normals[b+1]*normals[b+1] + normals[b+2]*normals[b+2]).sqrt();
            if len > 1e-10 {
                normals[b]   /= len;
                normals[b+1] /= len;
                normals[b+2] /= len;
            }
        }

        (positions, normals, indices)
    }

    /// Build from flat position + index arrays.
    pub fn from_buffers(positions: &[f32], indices: &[u32]) -> Self {
        let tri_count = indices.len() / 3;
        let vert_count = positions.len() / 3;
        let mut tris = Vec::with_capacity(tri_count * 9);
        let mut bboxes = Vec::with_capacity(tri_count * 6);
        let mut aabb_min = [f32::INFINITY; 3];
        let mut aabb_max = [f32::NEG_INFINITY; 3];

        for t in 0..tri_count {
            let i0 = indices[t * 3] as usize;
            let i1 = indices[t * 3 + 1] as usize;
            let i2 = indices[t * 3 + 2] as usize;
            if i0 >= vert_count || i1 >= vert_count || i2 >= vert_count {
                continue;
            }
            let mut tri_min = [f32::INFINITY; 3];
            let mut tri_max = [f32::NEG_INFINITY; 3];
            for &idx in &[i0, i1, i2] {
                for a in 0..3 {
                    let v = positions[idx * 3 + a];
                    tris.push(v);
                    tri_min[a] = tri_min[a].min(v);
                    tri_max[a] = tri_max[a].max(v);
                }
            }
            for a in 0..3 {
                aabb_min[a] = aabb_min[a].min(tri_min[a]);
                aabb_max[a] = aabb_max[a].max(tri_max[a]);
            }
            bboxes.extend_from_slice(&tri_min);
            bboxes.extend_from_slice(&tri_max);
        }

        if tris.is_empty() {
            aabb_min = [0.0; 3];
            aabb_max = [0.0; 3];
        }

        let actual_tri_count = tris.len() / 9;
        let (bvh_nodes, tri_order) = build_bvh(&tris, &bboxes, actual_tri_count);
        let sign_flip = compute_sign_flip(&tris);
        let fwn_nodes = build_fwn_data(&tris, &bvh_nodes, &tri_order);

        Self {
            tris,
            bboxes,
            aabb_min,
            aabb_max,
            bvh_nodes,
            tri_order,
            sign_flip,
            fwn_nodes,
        }
    }

    /// K13.3: manifold check.
    pub fn check_manifold(&self) -> ManifoldReport {
        let tri_count = self.tris.len() / 9;
        if tri_count == 0 {
            return ManifoldReport {
                triangle_count: 0,
                open_edges: 0,
                non_manifold_edges: 0,
                is_watertight: true,
            };
        }

        use std::collections::HashMap;
        let mut edge_counts: HashMap<(i64, i64, i64, i64, i64, i64), u32> = HashMap::new();
        let quantize = |v: f32| -> i64 { (v * 10000.0).round() as i64 };

        for t in 0..tri_count {
            let base = t * 9;
            let verts: [(i64, i64, i64); 3] = [
                (quantize(self.tris[base]), quantize(self.tris[base + 1]), quantize(self.tris[base + 2])),
                (quantize(self.tris[base + 3]), quantize(self.tris[base + 4]), quantize(self.tris[base + 5])),
                (quantize(self.tris[base + 6]), quantize(self.tris[base + 7]), quantize(self.tris[base + 8])),
            ];
            for &(a, b) in &[(0usize, 1usize), (1, 2), (2, 0)] {
                let edge = if verts[a] < verts[b] {
                    (verts[a].0, verts[a].1, verts[a].2, verts[b].0, verts[b].1, verts[b].2)
                } else {
                    (verts[b].0, verts[b].1, verts[b].2, verts[a].0, verts[a].1, verts[a].2)
                };
                *edge_counts.entry(edge).or_insert(0) += 1;
            }
        }

        let mut open = 0u32;
        let mut non_manifold = 0u32;
        for &count in edge_counts.values() {
            if count == 1 {
                open += 1;
            } else if count > 2 {
                non_manifold += 1;
            }
        }

        ManifoldReport {
            triangle_count: tri_count as u32,
            open_edges: open,
            non_manifold_edges: non_manifold,
            is_watertight: open == 0,
        }
    }

    /// K13: attempt to repair non-manifold meshes by filling holes.
    pub fn repair(&self) -> Self {
        use std::collections::HashMap;

        let tri_count = self.tris.len() / 9;
        if tri_count == 0 {
            return self.clone();
        }

        let quantize = |v: f32| -> i64 { (v * 10000.0).round() as i64 };
        type QVert = (i64, i64, i64);

        let mut edge_counts: HashMap<(QVert, QVert), u32> = HashMap::new();
        let mut coord_map: HashMap<QVert, [f32; 3]> = HashMap::new();

        for t in 0..tri_count {
            let base = t * 9;
            let verts_q: [QVert; 3] = [
                (quantize(self.tris[base]), quantize(self.tris[base + 1]), quantize(self.tris[base + 2])),
                (quantize(self.tris[base + 3]), quantize(self.tris[base + 4]), quantize(self.tris[base + 5])),
                (quantize(self.tris[base + 6]), quantize(self.tris[base + 7]), quantize(self.tris[base + 8])),
            ];
            let verts_f: [[f32; 3]; 3] = [
                [self.tris[base], self.tris[base + 1], self.tris[base + 2]],
                [self.tris[base + 3], self.tris[base + 4], self.tris[base + 5]],
                [self.tris[base + 6], self.tris[base + 7], self.tris[base + 8]],
            ];
            for &(a, b) in &[(0usize, 1usize), (1, 2), (2, 0)] {
                let key = if verts_q[a] < verts_q[b] { (verts_q[a], verts_q[b]) } else { (verts_q[b], verts_q[a]) };
                *edge_counts.entry(key).or_insert(0) += 1;
                coord_map.entry(verts_q[a]).or_insert(verts_f[a]);
                coord_map.entry(verts_q[b]).or_insert(verts_f[b]);
            }
        }

        let open_edges: Vec<(QVert, QVert)> = edge_counts
            .iter()
            .filter(|&(_, &c)| c == 1)
            .map(|(k, _)| *k)
            .collect();

        if open_edges.is_empty() {
            return self.clone();
        }

        let mut adj: HashMap<QVert, Vec<QVert>> = HashMap::new();
        for &(a, b) in &open_edges {
            adj.entry(a).or_default().push(b);
            adj.entry(b).or_default().push(a);
        }

        let mut visited: std::collections::HashSet<QVert> = std::collections::HashSet::new();
        let mut new_tris: Vec<f32> = Vec::new();

        for &(start, _) in &open_edges {
            if visited.contains(&start) { continue; }

            let mut loop_verts: Vec<QVert> = Vec::new();
            let mut curr = start;
            let mut prev: Option<QVert> = None;
            let mut ok = true;

            loop {
                if visited.contains(&curr) && curr != start {
                    ok = false; break;
                }
                loop_verts.push(curr);
                visited.insert(curr);

                let neighbors = match adj.get(&curr) {
                    Some(n) => n,
                    None => { ok = false; break; }
                };
                if neighbors.len() != 2 { ok = false; break; }

                let next = if Some(neighbors[0]) == prev { neighbors[1] } else { neighbors[0] };
                prev = Some(curr);
                curr = next;

                if curr == start { break; }
                if loop_verts.len() > open_edges.len() + 1 { ok = false; break; }
            }

            if !ok || loop_verts.len() < 3 { continue; }

            let coords: Vec<[f32; 3]> = loop_verts.iter()
                .map(|v| *coord_map.get(v).unwrap_or(&[0.0; 3]))
                .collect();

            let n = coords.len();
            let centroid = [
                coords.iter().map(|c| c[0]).sum::<f32>() / n as f32,
                coords.iter().map(|c| c[1]).sum::<f32>() / n as f32,
                coords.iter().map(|c| c[2]).sum::<f32>() / n as f32,
            ];

            for i in 0..n {
                let j = (i + 1) % n;
                new_tris.extend_from_slice(&coords[i]);
                new_tris.extend_from_slice(&coords[j]);
                new_tris.extend_from_slice(&centroid);
            }
        }

        if new_tris.is_empty() {
            return self.clone();
        }

        let mut all_positions = Vec::new();
        let mut all_indices = Vec::new();
        let total_tris = tri_count + new_tris.len() / 9;
        all_positions.reserve(total_tris * 9);
        all_indices.reserve(total_tris * 3);

        // Original tris as indexed mesh (identity indexing since tris are already flat)
        for t in 0..tri_count {
            let base = t * 9;
            for v in 0..3 {
                let vb = base + v * 3;
                all_indices.push((all_positions.len() / 3) as u32);
                all_positions.push(self.tris[vb]);
                all_positions.push(self.tris[vb + 1]);
                all_positions.push(self.tris[vb + 2]);
            }
        }
        // New hole-filling tris
        let new_tri_count = new_tris.len() / 9;
        for t in 0..new_tri_count {
            let base = t * 9;
            for v in 0..3 {
                let vb = base + v * 3;
                all_indices.push((all_positions.len() / 3) as u32);
                all_positions.push(new_tris[vb]);
                all_positions.push(new_tris[vb + 1]);
                all_positions.push(new_tris[vb + 2]);
            }
        }

        Self::from_buffers(&all_positions, &all_indices)
    }

    /// Bake the mesh SDF into a 3D volume grid for GPU texture upload.
    /// Resolution is per-axis (e.g. 64 → 64³ = 262k evaluations).
    pub fn bake_volume(&self, resolution: u32) -> BakedVolume {
        let pad_frac = 0.05;
        let extent = [
            self.aabb_max[0] - self.aabb_min[0],
            self.aabb_max[1] - self.aabb_min[1],
            self.aabb_max[2] - self.aabb_min[2],
        ];
        let max_ext = extent[0].max(extent[1]).max(extent[2]).max(1e-6);
        let padding = max_ext * pad_frac;
        let aabb_min = [
            self.aabb_min[0] - padding,
            self.aabb_min[1] - padding,
            self.aabb_min[2] - padding,
        ];
        let aabb_max = [
            self.aabb_max[0] + padding,
            self.aabb_max[1] + padding,
            self.aabb_max[2] + padding,
        ];
        let res = resolution as usize;
        let mut data = vec![0.0f32; res * res * res];
        let step = [
            (aabb_max[0] - aabb_min[0]) / resolution as f32,
            (aabb_max[1] - aabb_min[1]) / resolution as f32,
            (aabb_max[2] - aabb_min[2]) / resolution as f32,
        ];
        for z in 0..res {
            for y in 0..res {
                for x in 0..res {
                    let p = [
                        aabb_min[0] + (x as f32 + 0.5) * step[0],
                        aabb_min[1] + (y as f32 + 0.5) * step[1],
                        aabb_min[2] + (z as f32 + 0.5) * step[2],
                    ];
                    data[z * res * res + y * res + x] = self.sdf(p);
                }
            }
        }
        BakedVolume {
            data,
            dims: [resolution, resolution, resolution],
            aabb_min,
            aabb_max,
        }
    }

    /// Signed distance from `p` to the mesh surface.
    /// Negative inside, positive outside.
    pub fn sdf(&self, p: [f32; 3]) -> f32 {
        let tri_count = self.tris.len() / 9;
        if tri_count == 0 {
            return f32::INFINITY;
        }

        let (best_dist_sq, _best_tri, _best_s, _best_t) = if self.bvh_nodes.is_empty() {
            self.sdf_brute_force_dist_sq(p)
        } else {
            self.sdf_bvh_dist_sq(p)
        };

        let dist = best_dist_sq.sqrt();

        let mut sign = self.fast_winding_sign(p);
        if self.sign_flip {
            sign = -sign;
        }

        sign * dist
    }

    /// Sign via Fast Winding Numbers (Barill et al. 2018).
    /// Uses BVH with order-0 dipole approximation for O(log n) evaluation.
    fn fast_winding_sign(&self, p: [f32; 3]) -> f32 {
        let pd = [p[0] as f64, p[1] as f64, p[2] as f64];
        let omega = if self.bvh_nodes.is_empty() {
            self.brute_force_winding(pd)
        } else {
            self.fwn_query(pd, 0)
        };
        let w = omega / (4.0 * std::f64::consts::PI);
        if w > 0.5 { -1.0 } else { 1.0 }
    }

    fn brute_force_winding(&self, p: [f64; 3]) -> f64 {
        let tri_count = self.tris.len() / 9;
        let mut omega = 0.0f64;
        for t in 0..tri_count {
            omega += self.tri_solid_angle(p, t);
        }
        omega
    }

    fn fwn_query(&self, p: [f64; 3], node_idx: usize) -> f64 {
        let node = &self.bvh_nodes[node_idx];
        let fwn = &self.fwn_nodes[node_idx];

        if node.count > 0 {
            let mut omega = 0.0f64;
            for i in 0..node.count {
                let tri_idx = self.tri_order[(node.offset + i) as usize] as usize;
                omega += self.tri_solid_angle(p, tri_idx);
            }
            return omega;
        }

        let q = [
            fwn.center[0] - p[0],
            fwn.center[1] - p[1],
            fwn.center[2] - p[2],
        ];
        let dist_sq = q[0] * q[0] + q[1] * q[1] + q[2] * q[2];

        // beta=2.0: use dipole when distance > 2x cluster radius
        if dist_sq > 4.0 * fwn.radius_sq {
            let dist = dist_sq.sqrt();
            let inv_dist3 = 1.0 / (dist * dist_sq);
            let dot = q[0] * fwn.area_normal[0]
                + q[1] * fwn.area_normal[1]
                + q[2] * fwn.area_normal[2];
            return -dot * inv_dist3;
        }

        let left = node.offset as usize;
        self.fwn_query(p, left) + self.fwn_query(p, left + 1)
    }

    #[inline]
    fn tri_solid_angle(&self, p: [f64; 3], tri_idx: usize) -> f64 {
        let base = tri_idx * 9;
        let a = [
            self.tris[base] as f64 - p[0],
            self.tris[base + 1] as f64 - p[1],
            self.tris[base + 2] as f64 - p[2],
        ];
        let b = [
            self.tris[base + 3] as f64 - p[0],
            self.tris[base + 4] as f64 - p[1],
            self.tris[base + 5] as f64 - p[2],
        ];
        let c = [
            self.tris[base + 6] as f64 - p[0],
            self.tris[base + 7] as f64 - p[1],
            self.tris[base + 8] as f64 - p[2],
        ];

        let la = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt();
        let lb = (b[0] * b[0] + b[1] * b[1] + b[2] * b[2]).sqrt();
        let lc = (c[0] * c[0] + c[1] * c[1] + c[2] * c[2]).sqrt();

        if la < 1e-10 || lb < 1e-10 || lc < 1e-10 {
            return 2.0 * std::f64::consts::PI;
        }

        let num = a[0] * (b[1] * c[2] - b[2] * c[1])
            - a[1] * (b[0] * c[2] - b[2] * c[0])
            + a[2] * (b[0] * c[1] - b[1] * c[0]);

        let ab = a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
        let ac = a[0] * c[0] + a[1] * c[1] + a[2] * c[2];
        let bc = b[0] * c[0] + b[1] * c[1] + b[2] * c[2];

        let den = la * lb * lc + ab * lc + ac * lb + bc * la;
        2.0 * num.atan2(den)
    }

    fn sdf_brute_force_dist_sq(&self, p: [f32; 3]) -> (f32, usize, f32, f32) {
        let tri_count = self.tris.len() / 9;
        let mut best_dist_sq = f32::INFINITY;
        let mut best_tri = 0usize;
        let mut best_s = 0.0f32;
        let mut best_t = 0.0f32;
        for t in 0..tri_count {
            let base = t * 9;
            let v0 = [self.tris[base], self.tris[base + 1], self.tris[base + 2]];
            let v1 = [self.tris[base + 3], self.tris[base + 4], self.tris[base + 5]];
            let v2 = [self.tris[base + 6], self.tris[base + 7], self.tris[base + 8]];
            let (dist_sq, s, t_bary) = point_triangle_dist_sq(p, v0, v1, v2);
            if dist_sq < best_dist_sq {
                best_dist_sq = dist_sq;
                best_tri = t;
                best_s = s;
                best_t = t_bary;
            }
        }
        (best_dist_sq, best_tri, best_s, best_t)
    }

    fn sdf_bvh_dist_sq(&self, p: [f32; 3]) -> (f32, usize, f32, f32) {
        let mut best_dist_sq = f32::INFINITY;
        let mut best_tri = 0usize;
        let mut best_s = 0.0f32;
        let mut best_t = 0.0f32;
        let mut stack: Vec<u32> = Vec::with_capacity(64);
        stack.push(0);

        while let Some(node_idx) = stack.pop() {
            let node = &self.bvh_nodes[node_idx as usize];

            let aabb_dist_sq = aabb_point_dist_sq(p, node.aabb_min, node.aabb_max);
            if aabb_dist_sq >= best_dist_sq {
                continue;
            }

            if node.count > 0 {
                for i in 0..node.count {
                    let tri_idx = self.tri_order[(node.offset + i) as usize] as usize;
                    let base = tri_idx * 9;
                    let v0 = [self.tris[base], self.tris[base + 1], self.tris[base + 2]];
                    let v1 = [self.tris[base + 3], self.tris[base + 4], self.tris[base + 5]];
                    let v2 = [self.tris[base + 6], self.tris[base + 7], self.tris[base + 8]];
                    let (dist_sq, s, t_bary) = point_triangle_dist_sq(p, v0, v1, v2);
                    if dist_sq < best_dist_sq {
                        best_dist_sq = dist_sq;
                        best_tri = tri_idx;
                        best_s = s;
                        best_t = t_bary;
                    }
                }
            } else {
                let left = node.offset;
                let right = node.offset + 1;
                let left_dist = aabb_point_dist_sq(p, self.bvh_nodes[left as usize].aabb_min, self.bvh_nodes[left as usize].aabb_max);
                let right_dist = aabb_point_dist_sq(p, self.bvh_nodes[right as usize].aabb_min, self.bvh_nodes[right as usize].aabb_max);
                if left_dist < right_dist {
                    stack.push(right);
                    stack.push(left);
                } else {
                    stack.push(left);
                    stack.push(right);
                }
            }
        }

        (best_dist_sq, best_tri, best_s, best_t)
    }

}

// ─── BVH construction ────────────────────────────────────────

fn build_bvh(_tris: &[f32], bboxes: &[f32], tri_count: usize) -> (Vec<BvhNode>, Vec<u32>) {
    if tri_count == 0 {
        return (vec![], vec![]);
    }

    let mut tri_order: Vec<u32> = (0..tri_count as u32).collect();
    let mut nodes: Vec<BvhNode> = Vec::with_capacity(2 * tri_count);

    // Precompute centroids
    let mut centroids: Vec<[f32; 3]> = Vec::with_capacity(tri_count);
    for t in 0..tri_count {
        let bb = t * 6;
        centroids.push([
            (bboxes[bb] + bboxes[bb + 3]) * 0.5,
            (bboxes[bb + 1] + bboxes[bb + 4]) * 0.5,
            (bboxes[bb + 2] + bboxes[bb + 5]) * 0.5,
        ]);
    }

    // Push root placeholder
    nodes.push(BvhNode {
        aabb_min: [0.0; 3],
        aabb_max: [0.0; 3],
        offset: 0,
        count: 0,
    });

    struct BuildTask {
        node_idx: usize,
        start: usize,
        end: usize,
    }

    let mut tasks: Vec<BuildTask> = Vec::with_capacity(64);
    tasks.push(BuildTask { node_idx: 0, start: 0, end: tri_count });

    while let Some(task) = tasks.pop() {
        let start = task.start;
        let end = task.end;
        let count = end - start;

        // Compute AABB for this range
        let mut aabb_min = [f32::INFINITY; 3];
        let mut aabb_max = [f32::NEG_INFINITY; 3];
        for i in start..end {
            let t = tri_order[i] as usize;
            let bb = t * 6;
            for a in 0..3 {
                aabb_min[a] = aabb_min[a].min(bboxes[bb + a]);
                aabb_max[a] = aabb_max[a].max(bboxes[bb + 3 + a]);
            }
        }

        nodes[task.node_idx].aabb_min = aabb_min;
        nodes[task.node_idx].aabb_max = aabb_max;

        if count <= BVH_LEAF_SIZE {
            nodes[task.node_idx].offset = start as u32;
            nodes[task.node_idx].count = count as u32;
            continue;
        }

        // Split along longest axis at median centroid
        let extent = [
            aabb_max[0] - aabb_min[0],
            aabb_max[1] - aabb_min[1],
            aabb_max[2] - aabb_min[2],
        ];
        let axis = if extent[0] >= extent[1] && extent[0] >= extent[2] {
            0
        } else if extent[1] >= extent[2] {
            1
        } else {
            2
        };

        let mid = (start + end) / 2;
        tri_order[start..end].select_nth_unstable_by(mid - start, |&a, &b| {
            centroids[a as usize][axis]
                .partial_cmp(&centroids[b as usize][axis])
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let left_idx = nodes.len();
        let right_idx = left_idx + 1;
        nodes[task.node_idx].offset = left_idx as u32;
        nodes[task.node_idx].count = 0; // interior

        nodes.push(BvhNode { aabb_min: [0.0; 3], aabb_max: [0.0; 3], offset: 0, count: 0 });
        nodes.push(BvhNode { aabb_min: [0.0; 3], aabb_max: [0.0; 3], offset: 0, count: 0 });

        tasks.push(BuildTask { node_idx: right_idx, start: mid, end });
        tasks.push(BuildTask { node_idx: left_idx, start, end: mid });
    }

    (nodes, tri_order)
}

// ─── Fast Winding Number construction (Barill et al. 2018) ───

fn build_fwn_data(
    tris: &[f32],
    bvh_nodes: &[BvhNode],
    tri_order: &[u32],
) -> Vec<FwnNode> {
    if bvh_nodes.is_empty() {
        return vec![];
    }

    let mut fwn: Vec<FwnNode> = vec![
        FwnNode {
            center: [0.0; 3],
            area_normal: [0.0; 3],
            radius_sq: 0.0,
        };
        bvh_nodes.len()
    ];

    // Bottom-up: process in reverse order (children before parents).
    for ni in (0..bvh_nodes.len()).rev() {
        let node = &bvh_nodes[ni];

        if node.count > 0 {
            // Leaf: accumulate from triangles directly.
            let mut total_area = 0.0f64;
            let mut center = [0.0f64; 3];
            let mut area_normal = [0.0f64; 3];

            for i in 0..node.count {
                let tri_idx = tri_order[(node.offset + i) as usize] as usize;
                let base = tri_idx * 9;
                let v0 = [tris[base] as f64, tris[base + 1] as f64, tris[base + 2] as f64];
                let v1 = [tris[base + 3] as f64, tris[base + 4] as f64, tris[base + 5] as f64];
                let v2 = [tris[base + 6] as f64, tris[base + 7] as f64, tris[base + 8] as f64];

                let e1 = [v1[0] - v0[0], v1[1] - v0[1], v1[2] - v0[2]];
                let e2 = [v2[0] - v0[0], v2[1] - v0[1], v2[2] - v0[2]];
                // area_normal = 0.5 * cross(e1, e2)
                let an = [
                    0.5 * (e1[1] * e2[2] - e1[2] * e2[1]),
                    0.5 * (e1[2] * e2[0] - e1[0] * e2[2]),
                    0.5 * (e1[0] * e2[1] - e1[1] * e2[0]),
                ];
                let area = (an[0] * an[0] + an[1] * an[1] + an[2] * an[2]).sqrt();

                let centroid = [
                    (v0[0] + v1[0] + v2[0]) / 3.0,
                    (v0[1] + v1[1] + v2[1]) / 3.0,
                    (v0[2] + v1[2] + v2[2]) / 3.0,
                ];

                center[0] += area * centroid[0];
                center[1] += area * centroid[1];
                center[2] += area * centroid[2];
                area_normal[0] += an[0];
                area_normal[1] += an[1];
                area_normal[2] += an[2];
                total_area += area;
            }

            if total_area > 1e-20 {
                center[0] /= total_area;
                center[1] /= total_area;
                center[2] /= total_area;
            }

            // radius_sq: max squared distance from center to any vertex
            let mut max_r2 = 0.0f64;
            for i in 0..node.count {
                let tri_idx = tri_order[(node.offset + i) as usize] as usize;
                let base = tri_idx * 9;
                for v in 0..3 {
                    let vb = base + v * 3;
                    let dx = tris[vb] as f64 - center[0];
                    let dy = tris[vb + 1] as f64 - center[1];
                    let dz = tris[vb + 2] as f64 - center[2];
                    let r2 = dx * dx + dy * dy + dz * dz;
                    if r2 > max_r2 {
                        max_r2 = r2;
                    }
                }
            }

            fwn[ni] = FwnNode {
                center,
                area_normal,
                radius_sq: max_r2,
            };
        } else {
            // Interior: merge children.
            let left = node.offset as usize;
            let right = left + 1;
            let fl = &fwn[left];
            let fr = &fwn[right];

            let area_normal = [
                fl.area_normal[0] + fr.area_normal[0],
                fl.area_normal[1] + fr.area_normal[1],
                fl.area_normal[2] + fr.area_normal[2],
            ];

            // Area-weighted center: weight by |area_normal| of each child
            let wl = (fl.area_normal[0] * fl.area_normal[0]
                + fl.area_normal[1] * fl.area_normal[1]
                + fl.area_normal[2] * fl.area_normal[2])
            .sqrt();
            let wr = (fr.area_normal[0] * fr.area_normal[0]
                + fr.area_normal[1] * fr.area_normal[1]
                + fr.area_normal[2] * fr.area_normal[2])
            .sqrt();
            let wt = wl + wr;
            let center = if wt > 1e-20 {
                [
                    (wl * fl.center[0] + wr * fr.center[0]) / wt,
                    (wl * fl.center[1] + wr * fr.center[1]) / wt,
                    (wl * fl.center[2] + wr * fr.center[2]) / wt,
                ]
            } else {
                [
                    (fl.center[0] + fr.center[0]) * 0.5,
                    (fl.center[1] + fr.center[1]) * 0.5,
                    (fl.center[2] + fr.center[2]) * 0.5,
                ]
            };

            // radius_sq: max of (child_center_to_new_center + child_radius)²
            let dl = {
                let dx = fl.center[0] - center[0];
                let dy = fl.center[1] - center[1];
                let dz = fl.center[2] - center[2];
                (dx * dx + dy * dy + dz * dz).sqrt() + fl.radius_sq.sqrt()
            };
            let dr = {
                let dx = fr.center[0] - center[0];
                let dy = fr.center[1] - center[1];
                let dz = fr.center[2] - center[2];
                (dx * dx + dy * dy + dz * dz).sqrt() + fr.radius_sq.sqrt()
            };
            let radius = if dl > dr { dl } else { dr };

            fwn[ni] = FwnNode {
                center,
                area_normal,
                radius_sq: radius * radius,
            };
        }
    }

    fwn
}

// ─── Geometry helpers ────────────────────────────────────────

/// Squared distance from point to AABB (0 if inside).
#[inline]
fn aabb_point_dist_sq(p: [f32; 3], aabb_min: [f32; 3], aabb_max: [f32; 3]) -> f32 {
    let mut dist_sq = 0.0f32;
    for a in 0..3 {
        if p[a] < aabb_min[a] {
            let d = aabb_min[a] - p[a];
            dist_sq += d * d;
        } else if p[a] > aabb_max[a] {
            let d = p[a] - aabb_max[a];
            dist_sq += d * d;
        }
    }
    dist_sq
}

/// Squared distance from point `p` to triangle `(v0, v1, v2)`,
/// plus barycentric (s, t): closest = v0 + s*(v1-v0) + t*(v2-v0).
fn point_triangle_dist_sq(
    p: [f32; 3],
    v0: [f32; 3],
    v1: [f32; 3],
    v2: [f32; 3],
) -> (f32, f32, f32) {
    let e0 = sub3(v1, v0);
    let e1 = sub3(v2, v0);
    let v = sub3(p, v0);

    let a = dot3(e0, e0);
    let b = dot3(e0, e1);
    let c = dot3(e1, e1);
    let d = dot3(e0, v);
    let ee = dot3(e1, v);

    let det = a * c - b * b;
    let mut s = c * d - b * ee;
    let mut t = a * ee - b * d;

    if s + t <= det {
        if s < 0.0 {
            if t < 0.0 {
                if d > 0.0 {
                    t = 0.0;
                    s = if d >= a { 1.0 } else { d / a };
                } else {
                    s = 0.0;
                    t = if ee <= 0.0 { 0.0 } else if ee >= c { 1.0 } else { ee / c };
                }
            } else {
                s = 0.0;
                t = if ee <= 0.0 { 0.0 } else if ee >= c { 1.0 } else { ee / c };
            }
        } else if t < 0.0 {
            t = 0.0;
            s = if d <= 0.0 { 0.0 } else if d >= a { 1.0 } else { d / a };
        } else {
            if det.abs() < 1e-12 {
                s = 0.0;
                t = 0.0;
            } else {
                let inv = 1.0 / det;
                s *= inv;
                t *= inv;
            }
        }
    } else if s < 0.0 {
        let tmp0 = b - d;
        let tmp1 = c - ee;
        if tmp1 > tmp0 {
            let numer = tmp1 - tmp0;
            let denom = a - 2.0 * b + c;
            s = if numer >= denom { 1.0 } else { (numer / denom).max(0.0) };
            t = 1.0 - s;
        } else {
            s = 0.0;
            t = if ee <= 0.0 { 0.0 } else if ee >= c { 1.0 } else { ee / c };
        }
    } else if t < 0.0 {
        let tmp0 = b - ee;
        let tmp1 = a - d;
        if tmp1 > tmp0 {
            let numer = tmp1 - tmp0;
            let denom = a - 2.0 * b + c;
            t = if numer >= denom { 1.0 } else { (numer / denom).max(0.0) };
            s = 1.0 - t;
        } else {
            t = 0.0;
            s = if d <= 0.0 { 0.0 } else if d >= a { 1.0 } else { d / a };
        }
    } else {
        let numer = (c - ee) - (b - d);
        if numer <= 0.0 {
            s = 0.0;
            t = 1.0;
        } else {
            let denom = a - 2.0 * b + c;
            s = if numer >= denom { 1.0 } else { numer / denom };
            t = 1.0 - s;
        }
    }

    let closest = [
        v0[0] + s * e0[0] + t * e1[0],
        v0[1] + s * e0[1] + t * e1[1],
        v0[2] + s * e0[2] + t * e1[2],
    ];
    let diff = sub3(p, closest);
    (dot3(diff, diff), s, t)
}

#[inline]
fn sub3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

#[inline]
fn dot3(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Detect whether face normals point inward (CW winding) by
/// computing the signed volume of the mesh. Negative → CW.
fn compute_sign_flip(tris: &[f32]) -> bool {
    let tri_count = tris.len() / 9;
    let mut vol = 0.0f64;
    for t in 0..tri_count {
        let b = t * 9;
        let (x0, y0, z0) = (tris[b] as f64, tris[b + 1] as f64, tris[b + 2] as f64);
        let (x1, y1, z1) = (tris[b + 3] as f64, tris[b + 4] as f64, tris[b + 5] as f64);
        let (x2, y2, z2) = (tris[b + 6] as f64, tris[b + 7] as f64, tris[b + 8] as f64);
        vol += x0 * (y1 * z2 - y2 * z1)
             - y0 * (x1 * z2 - x2 * z1)
             + z0 * (x1 * y2 - x2 * y1);
    }
    vol < 0.0
}


// ─── Tests ───────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn unit_cube_mesh() -> MeshSdf {
        #[rustfmt::skip]
        let positions: Vec<f32> = vec![
            -1.0,-1.0,-1.0,  1.0,-1.0,-1.0,  1.0, 1.0,-1.0, -1.0, 1.0,-1.0,
            -1.0,-1.0, 1.0,  1.0,-1.0, 1.0,  1.0, 1.0, 1.0, -1.0, 1.0, 1.0,
        ];
        #[rustfmt::skip]
        let indices: Vec<u32> = vec![
            0,2,1, 0,3,2,
            4,5,6, 4,6,7,
            0,1,5, 0,5,4,
            3,6,2, 3,7,6,
            0,4,7, 0,7,3,
            1,2,6, 1,6,5,
        ];
        MeshSdf::from_buffers(&positions, &indices)
    }

    #[test]
    fn cube_mesh_is_watertight() {
        let m = unit_cube_mesh();
        let report = m.check_manifold();
        assert_eq!(report.triangle_count, 12);
        assert_eq!(report.open_edges, 0);
        assert_eq!(report.non_manifold_edges, 0);
        assert!(report.is_watertight);
    }

    #[test]
    fn cube_mesh_aabb_is_unit() {
        let m = unit_cube_mesh();
        for a in 0..3 {
            assert!((m.aabb_min[a] - (-1.0)).abs() < 1e-5);
            assert!((m.aabb_max[a] - 1.0).abs() < 1e-5);
        }
    }

    #[test]
    fn cube_mesh_surface_distance_is_near_zero() {
        let m = unit_cube_mesh();
        assert!(m.sdf([1.0, 0.0, 0.0]).abs() < 0.1);
    }

    #[test]
    fn cube_mesh_inside_is_negative() {
        let m = unit_cube_mesh();
        assert!(m.sdf([0.0, 0.0, 0.0]) < 0.0);
    }

    #[test]
    fn cube_mesh_outside_is_positive() {
        let m = unit_cube_mesh();
        assert!(m.sdf([3.0, 0.0, 0.0]) > 0.0);
    }

    fn cube_with_missing_face() -> MeshSdf {
        #[rustfmt::skip]
        let positions: Vec<f32> = vec![
            -1.0,-1.0,-1.0,  1.0,-1.0,-1.0,  1.0, 1.0,-1.0, -1.0, 1.0,-1.0,
            -1.0,-1.0, 1.0,  1.0,-1.0, 1.0,  1.0, 1.0, 1.0, -1.0, 1.0, 1.0,
        ];
        #[rustfmt::skip]
        let indices: Vec<u32> = vec![
            0,2,1, 0,3,2,
            0,1,5, 0,5,4,
            3,6,2, 3,7,6,
            0,4,7, 0,7,3,
            1,2,6, 1,6,5,
        ];
        MeshSdf::from_buffers(&positions, &indices)
    }

    #[test]
    fn k13_cube_with_hole_is_not_watertight() {
        let m = cube_with_missing_face();
        let report = m.check_manifold();
        assert!(!report.is_watertight);
        assert!(report.open_edges > 0);
    }

    #[test]
    fn k13_repair_fills_hole() {
        let m = cube_with_missing_face();
        let repaired = m.repair();
        let report = repaired.check_manifold();
        assert!(
            report.is_watertight,
            "K13: repaired mesh should be watertight, but has {} open edges",
            report.open_edges,
        );
        assert!(
            repaired.check_manifold().triangle_count > m.check_manifold().triangle_count,
            "K13: repair should add triangles",
        );
    }

    #[test]
    fn k13_watertight_mesh_unchanged_by_repair() {
        let m = unit_cube_mesh();
        let repaired = m.repair();
        assert_eq!(
            repaired.check_manifold().triangle_count,
            m.check_manifold().triangle_count,
        );
    }

    #[test]
    fn bvh_is_built_on_construction() {
        let m = unit_cube_mesh();
        assert!(!m.bvh_nodes.is_empty());
        assert_eq!(m.tri_order.len(), 12);
    }

    #[test]
    fn bake_volume_captures_inside_outside() {
        let m = unit_cube_mesh();
        let vol = m.bake_volume(16);
        assert_eq!(vol.dims, [16, 16, 16]);
        assert_eq!(vol.data.len(), 16 * 16 * 16);
        let center_val = vol.sample_trilinear([0.0, 0.0, 0.0]);
        assert!(center_val < 0.0, "center should be inside: {center_val}");
        let outside_val = vol.sample_trilinear([3.0, 0.0, 0.0]);
        assert!(outside_val > 0.0, "outside should be positive: {outside_val}");
    }

    #[test]
    fn bake_volume_trilinear_matches_sdf() {
        let m = unit_cube_mesh();
        let vol = m.bake_volume(32);
        let points = [[0.0, 0.0, 0.0], [0.5, 0.5, 0.5], [0.8, 0.0, 0.0]];
        for p in &points {
            let exact = m.sdf(*p);
            let approx = vol.sample_trilinear(*p);
            assert!(
                (exact - approx).abs() < 0.3,
                "trilinear mismatch at {p:?}: exact={exact}, approx={approx}",
            );
        }
    }

    #[test]
    fn bvh_sdf_matches_brute_force() {
        let m = unit_cube_mesh();
        let points = [
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [3.0, 0.0, 0.0],
            [0.5, 0.5, 0.5],
            [-2.0, -2.0, -2.0],
        ];
        for p in &points {
            let (bvh_dist, _, _, _) = m.sdf_bvh_dist_sq(*p);
            let (brute_dist, _, _, _) = m.sdf_brute_force_dist_sq(*p);
            assert!(
                (bvh_dist - brute_dist).abs() < 1e-6,
                "BVH vs brute mismatch at {:?}: {} vs {}",
                p, bvh_dist, brute_dist,
            );
        }
    }

    #[test]
    #[ignore]
    fn bench_sdf_query_time() {
        use std::collections::HashMap;
        let phi = (1.0 + 5.0_f32.sqrt()) / 2.0;
        let mut verts: Vec<[f32; 3]> = vec![
            [-1.0, phi, 0.0], [1.0, phi, 0.0], [-1.0, -phi, 0.0], [1.0, -phi, 0.0],
            [0.0, -1.0, phi], [0.0, 1.0, phi], [0.0, -1.0, -phi], [0.0, 1.0, -phi],
            [phi, 0.0, -1.0], [phi, 0.0, 1.0], [-phi, 0.0, -1.0], [-phi, 0.0, 1.0],
        ];
        for p in &mut verts {
            let len = (p[0]*p[0] + p[1]*p[1] + p[2]*p[2]).sqrt();
            p[0] /= len; p[1] /= len; p[2] /= len;
        }
        let mut tris: Vec<[u32; 3]> = vec![
            [0,11,5],[0,5,1],[0,1,7],[0,7,10],[0,10,11],
            [1,5,9],[5,11,4],[11,10,2],[10,7,6],[7,1,8],
            [3,9,4],[3,4,2],[3,2,6],[3,6,8],[3,8,9],
            [4,9,5],[2,4,11],[6,2,10],[8,6,7],[9,8,1],
        ];
        let mut cache: HashMap<(u32, u32), u32> = HashMap::new();
        for _ in 0..5 {
            let mut new_tris = Vec::with_capacity(tris.len() * 4);
            cache.clear();
            for tri in &tris {
                let mut mid = |a: u32, b: u32| -> u32 {
                    let key = if a < b { (a, b) } else { (b, a) };
                    *cache.entry(key).or_insert_with(|| {
                        let pa = verts[a as usize]; let pb = verts[b as usize];
                        let mut m = [(pa[0]+pb[0])/2.0, (pa[1]+pb[1])/2.0, (pa[2]+pb[2])/2.0];
                        let l = (m[0]*m[0]+m[1]*m[1]+m[2]*m[2]).sqrt();
                        m[0]/=l; m[1]/=l; m[2]/=l;
                        let idx = verts.len() as u32;
                        verts.push(m);
                        idx
                    })
                };
                let a = mid(tri[0], tri[1]);
                let b = mid(tri[1], tri[2]);
                let c = mid(tri[2], tri[0]);
                new_tris.push([tri[0], a, c]);
                new_tris.push([tri[1], b, a]);
                new_tris.push([tri[2], c, b]);
                new_tris.push([a, b, c]);
            }
            tris = new_tris;
        }
        let positions: Vec<f32> = verts.iter().flat_map(|v| v.iter().copied()).collect();
        let indices: Vec<u32> = tris.iter().flat_map(|t| t.iter().copied()).collect();
        let mesh = MeshSdf::from_buffers(&positions, &indices);
        let tri_count = mesh.tri_count();
        eprintln!("Sphere mesh: {} triangles", tri_count);

        let res = 32_u32;
        let total = res * res * res;
        let start = std::time::Instant::now();
        let mut sum = 0.0_f32;
        for iz in 0..res {
            for iy in 0..res {
                for ix in 0..res {
                    let p = [
                        -1.5 + 3.0 * (ix as f32) / (res as f32 - 1.0),
                        -1.5 + 3.0 * (iy as f32) / (res as f32 - 1.0),
                        -1.5 + 3.0 * (iz as f32) / (res as f32 - 1.0),
                    ];
                    sum += mesh.sdf(p);
                }
            }
        }
        let elapsed = start.elapsed();
        eprintln!("BVH: DC grid {}^3 = {} queries on {} tris: {:?}", res, total, tri_count, elapsed);
        eprintln!("Per query: {:?}", elapsed / total);
        eprintln!("(sum={} to prevent opt-out)", sum);
    }
}
