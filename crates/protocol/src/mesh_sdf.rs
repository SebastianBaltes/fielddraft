//! K13.0: Mesh-to-SDF — evaluate the signed distance from a
//! point to an imported triangle mesh. Uses a BVH (bounding
//! volume hierarchy) for O(log N) nearest-triangle queries,
//! and a ray-parity sign test for inside/outside classification.
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
        Ok(Self {
            tris: raw.tris,
            bboxes: raw.bboxes,
            aabb_min: raw.aabb_min,
            aabb_max: raw.aabb_max,
            bvh_nodes,
            tri_order,
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

        Self {
            tris,
            bboxes,
            aabb_min,
            aabb_max,
            bvh_nodes,
            tri_order,
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

    /// Signed distance from `p` to the mesh surface.
    /// Negative inside, positive outside.
    pub fn sdf(&self, p: [f32; 3]) -> f32 {
        let tri_count = self.tris.len() / 9;
        if tri_count == 0 {
            return f32::INFINITY;
        }

        let best_dist_sq = if self.bvh_nodes.is_empty() {
            self.sdf_brute_force_dist_sq(p)
        } else {
            self.sdf_bvh_dist_sq(p)
        };

        let dist = best_dist_sq.sqrt();
        let sign = if self.ray_parity(p) { -1.0 } else { 1.0 };
        sign * dist
    }

    fn sdf_brute_force_dist_sq(&self, p: [f32; 3]) -> f32 {
        let tri_count = self.tris.len() / 9;
        let mut best_dist_sq = f32::INFINITY;
        for t in 0..tri_count {
            let base = t * 9;
            let v0 = [self.tris[base], self.tris[base + 1], self.tris[base + 2]];
            let v1 = [self.tris[base + 3], self.tris[base + 4], self.tris[base + 5]];
            let v2 = [self.tris[base + 6], self.tris[base + 7], self.tris[base + 8]];
            let dist_sq = point_triangle_dist_sq(p, v0, v1, v2);
            if dist_sq < best_dist_sq {
                best_dist_sq = dist_sq;
            }
        }
        best_dist_sq
    }

    fn sdf_bvh_dist_sq(&self, p: [f32; 3]) -> f32 {
        let mut best_dist_sq = f32::INFINITY;
        let mut stack: Vec<u32> = Vec::with_capacity(64);
        stack.push(0);

        while let Some(node_idx) = stack.pop() {
            let node = &self.bvh_nodes[node_idx as usize];

            let aabb_dist_sq = aabb_point_dist_sq(p, node.aabb_min, node.aabb_max);
            if aabb_dist_sq >= best_dist_sq {
                continue;
            }

            if node.count > 0 {
                // Leaf: test all triangles
                for i in 0..node.count {
                    let t = self.tri_order[(node.offset + i) as usize] as usize;
                    let base = t * 9;
                    let v0 = [self.tris[base], self.tris[base + 1], self.tris[base + 2]];
                    let v1 = [self.tris[base + 3], self.tris[base + 4], self.tris[base + 5]];
                    let v2 = [self.tris[base + 6], self.tris[base + 7], self.tris[base + 8]];
                    let dist_sq = point_triangle_dist_sq(p, v0, v1, v2);
                    if dist_sq < best_dist_sq {
                        best_dist_sq = dist_sq;
                    }
                }
            } else {
                // Interior: push children (nearer child last so it's popped first)
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

        best_dist_sq
    }

    /// Ray-parity inside test: cast a ray along +Y and count
    /// intersections. Odd = inside.
    fn ray_parity(&self, p: [f32; 3]) -> bool {
        let p = [p[0] + 1.37e-5, p[1], p[2] + 2.19e-5];

        if self.bvh_nodes.is_empty() {
            return self.ray_parity_brute_force(p);
        }

        let mut crossings = 0u32;
        let mut stack: Vec<u32> = Vec::with_capacity(64);
        stack.push(0);

        while let Some(node_idx) = stack.pop() {
            let node = &self.bvh_nodes[node_idx as usize];

            // Ray along +Y: origin = p, direction = (0, 1, 0).
            // The ray hits the AABB if p.xz is inside the AABB's xz extent
            // AND the AABB's max.y >= p.y (ray goes upward).
            if p[0] < node.aabb_min[0] || p[0] > node.aabb_max[0]
                || p[2] < node.aabb_min[2] || p[2] > node.aabb_max[2]
                || node.aabb_max[1] < p[1]
            {
                continue;
            }

            if node.count > 0 {
                for i in 0..node.count {
                    let t = self.tri_order[(node.offset + i) as usize] as usize;
                    if self.ray_hits_triangle(p, t) {
                        crossings += 1;
                    }
                }
            } else {
                stack.push(node.offset);
                stack.push(node.offset + 1);
            }
        }

        crossings % 2 == 1
    }

    fn ray_parity_brute_force(&self, p: [f32; 3]) -> bool {
        let tri_count = self.tris.len() / 9;
        let mut crossings = 0u32;
        for t in 0..tri_count {
            if self.ray_hits_triangle(p, t) {
                crossings += 1;
            }
        }
        crossings % 2 == 1
    }

    /// Test if a +Y ray from `p` intersects triangle `t`.
    #[inline]
    fn ray_hits_triangle(&self, p: [f32; 3], t: usize) -> bool {
        let base = t * 9;
        let v0 = [self.tris[base], self.tris[base + 1], self.tris[base + 2]];
        let v1 = [self.tris[base + 3], self.tris[base + 4], self.tris[base + 5]];
        let v2 = [self.tris[base + 6], self.tris[base + 7], self.tris[base + 8]];

        let (ax, az) = (v0[0] - p[0], v0[2] - p[2]);
        let (bx, bz) = (v1[0] - p[0], v1[2] - p[2]);
        let (cx, cz) = (v2[0] - p[0], v2[2] - p[2]);

        let d0 = ax * bz - az * bx;
        let d1 = bx * cz - bz * cx;
        let d2 = cx * az - cz * ax;

        let all_pos = d0 > 0.0 && d1 > 0.0 && d2 > 0.0;
        let all_neg = d0 < 0.0 && d1 < 0.0 && d2 < 0.0;
        if !all_pos && !all_neg {
            return false;
        }

        let denom = d0 + d1 + d2;
        if denom.abs() < 1e-12 {
            return false;
        }
        let u = d1 / denom;
        let v = d2 / denom;
        let w = 1.0 - u - v;
        let y_hit = w * v0[1] + u * v1[1] + v * v2[1];
        y_hit > p[1]
    }
}

// ─── BVH construction ────────────────────────────────────────

fn build_bvh(tris: &[f32], bboxes: &[f32], tri_count: usize) -> (Vec<BvhNode>, Vec<u32>) {
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

/// Squared distance from point `p` to triangle `(v0, v1, v2)`.
fn point_triangle_dist_sq(
    p: [f32; 3],
    v0: [f32; 3],
    v1: [f32; 3],
    v2: [f32; 3],
) -> f32 {
    let e0 = sub3(v1, v0);
    let e1 = sub3(v2, v0);
    let v = sub3(p, v0);

    let d00 = dot3(e0, e0);
    let d01 = dot3(e0, e1);
    let d11 = dot3(e1, e1);
    let d20 = dot3(v, e0);
    let d21 = dot3(v, e1);

    let denom = d00 * d11 - d01 * d01;
    if denom.abs() < 1e-12 {
        return dot3(v, v);
    }

    let inv = 1.0 / denom;
    let mut s = (d11 * d20 - d01 * d21) * inv;
    let mut t = (d00 * d21 - d01 * d20) * inv;

    if s < 0.0 { s = 0.0; }
    if t < 0.0 { t = 0.0; }
    if s + t > 1.0 {
        let scale = 1.0 / (s + t);
        s *= scale;
        t *= scale;
    }

    let closest = [
        v0[0] + s * e0[0] + t * e1[0],
        v0[1] + s * e0[1] + t * e1[1],
        v0[2] + s * e0[2] + t * e1[2],
    ];
    let diff = sub3(p, closest);
    dot3(diff, diff)
}

#[inline]
fn sub3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

#[inline]
fn dot3(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
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
            let bvh_result = m.sdf_bvh_dist_sq(*p);
            let brute_result = m.sdf_brute_force_dist_sq(*p);
            assert!(
                (bvh_result - brute_result).abs() < 1e-6,
                "BVH vs brute mismatch at {:?}: {} vs {}",
                p, bvh_result, brute_result,
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
