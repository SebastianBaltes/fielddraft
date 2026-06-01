//! Lightweight triangle BVH for closest-point-on-mesh queries.
//! Used by the fine-tuning loop to project sphere centers onto the
//! evolving sPSR mesh.

use fielddraft_protocol::mesh_sdf::point_triangle;

const BVH_LEAF: usize = 4;

struct CpNode {
    aabb_min: [f32; 3],
    aabb_max: [f32; 3],
    offset: u32,
    count: u32,
}

pub struct ClosestPointBvh {
    nodes: Vec<CpNode>,
    order: Vec<u32>,
    verts: Vec<f32>,
}

impl ClosestPointBvh {
    /// Build from flat position buffer and flat triangle index buffer.
    pub fn build(positions: &[f32], indices: &[u32]) -> Self {
        let tri_count = indices.len() / 3;
        if tri_count == 0 {
            return Self {
                nodes: vec![],
                order: vec![],
                verts: positions.to_vec(),
            };
        }

        let mut bboxes = Vec::with_capacity(tri_count * 6);
        let mut centroids = Vec::with_capacity(tri_count);
        for t in 0..tri_count {
            let i0 = indices[t * 3] as usize;
            let i1 = indices[t * 3 + 1] as usize;
            let i2 = indices[t * 3 + 2] as usize;
            let (v0, v1, v2) = (
                [positions[i0 * 3], positions[i0 * 3 + 1], positions[i0 * 3 + 2]],
                [positions[i1 * 3], positions[i1 * 3 + 1], positions[i1 * 3 + 2]],
                [positions[i2 * 3], positions[i2 * 3 + 1], positions[i2 * 3 + 2]],
            );
            let mn = [
                v0[0].min(v1[0]).min(v2[0]),
                v0[1].min(v1[1]).min(v2[1]),
                v0[2].min(v1[2]).min(v2[2]),
            ];
            let mx = [
                v0[0].max(v1[0]).max(v2[0]),
                v0[1].max(v1[1]).max(v2[1]),
                v0[2].max(v1[2]).max(v2[2]),
            ];
            bboxes.extend_from_slice(&mn);
            bboxes.extend_from_slice(&mx);
            centroids.push([
                (mn[0] + mx[0]) * 0.5,
                (mn[1] + mx[1]) * 0.5,
                (mn[2] + mx[2]) * 0.5,
            ]);
        }

        let mut order: Vec<u32> = (0..tri_count as u32).collect();
        let mut nodes: Vec<CpNode> = Vec::with_capacity(2 * tri_count);
        nodes.push(CpNode {
            aabb_min: [0.0; 3],
            aabb_max: [0.0; 3],
            offset: 0,
            count: 0,
        });

        struct Task {
            node_idx: usize,
            start: usize,
            end: usize,
        }

        let mut tasks = vec![Task { node_idx: 0, start: 0, end: tri_count }];

        while let Some(task) = tasks.pop() {
            let (start, end) = (task.start, task.end);
            let count = end - start;

            let mut amin = [f32::INFINITY; 3];
            let mut amax = [f32::NEG_INFINITY; 3];
            for &oi in &order[start..end] {
                let t = oi as usize;
                let bb = t * 6;
                for a in 0..3 {
                    amin[a] = amin[a].min(bboxes[bb + a]);
                    amax[a] = amax[a].max(bboxes[bb + 3 + a]);
                }
            }
            nodes[task.node_idx].aabb_min = amin;
            nodes[task.node_idx].aabb_max = amax;

            if count <= BVH_LEAF {
                nodes[task.node_idx].offset = start as u32;
                nodes[task.node_idx].count = count as u32;
                continue;
            }

            let extent = [amax[0] - amin[0], amax[1] - amin[1], amax[2] - amin[2]];
            let axis = if extent[0] >= extent[1] && extent[0] >= extent[2] {
                0
            } else if extent[1] >= extent[2] {
                1
            } else {
                2
            };

            let mid = (start + end) / 2;
            order[start..end].select_nth_unstable_by(mid - start, |&a, &b| {
                centroids[a as usize][axis]
                    .partial_cmp(&centroids[b as usize][axis])
                    .unwrap_or(std::cmp::Ordering::Equal)
            });

            let left_idx = nodes.len();
            nodes.push(CpNode {
                aabb_min: [0.0; 3],
                aabb_max: [0.0; 3],
                offset: 0,
                count: 0,
            });
            nodes.push(CpNode {
                aabb_min: [0.0; 3],
                aabb_max: [0.0; 3],
                offset: 0,
                count: 0,
            });
            nodes[task.node_idx].offset = left_idx as u32;
            nodes[task.node_idx].count = 0;

            tasks.push(Task { node_idx: left_idx + 1, start: mid, end });
            tasks.push(Task { node_idx: left_idx, start, end: mid });
        }

        // Store flat triangle vertex data for fast lookup
        let mut tri_verts = Vec::with_capacity(tri_count * 9);
        for t in 0..tri_count {
            let i0 = indices[t * 3] as usize;
            let i1 = indices[t * 3 + 1] as usize;
            let i2 = indices[t * 3 + 2] as usize;
            tri_verts.extend_from_slice(&positions[i0 * 3..i0 * 3 + 3]);
            tri_verts.extend_from_slice(&positions[i1 * 3..i1 * 3 + 3]);
            tri_verts.extend_from_slice(&positions[i2 * 3..i2 * 3 + 3]);
        }

        Self {
            nodes,
            order,
            verts: tri_verts,
        }
    }

    /// Find the closest point on the mesh to `p`.
    /// Returns (closest_point, signed_distance_estimate).
    /// The sign is positive if the query point is "outside" (heuristic).
    pub fn closest_point(&self, p: [f32; 3]) -> ([f32; 3], f32) {
        if self.nodes.is_empty() {
            return (p, f32::INFINITY);
        }

        let mut best_dist_sq = f32::INFINITY;
        let mut best_point = p;

        let mut stack = vec![0usize];
        while let Some(ni) = stack.pop() {
            let node = &self.nodes[ni];
            let box_dist = aabb_dist_sq(p, node.aabb_min, node.aabb_max);
            if box_dist > best_dist_sq {
                continue;
            }
            if node.count > 0 {
                for i in node.offset as usize..(node.offset + node.count) as usize {
                    let ti = self.order[i] as usize;
                    let b = ti * 9;
                    let v0 = [self.verts[b], self.verts[b + 1], self.verts[b + 2]];
                    let v1 = [self.verts[b + 3], self.verts[b + 4], self.verts[b + 5]];
                    let v2 = [self.verts[b + 6], self.verts[b + 7], self.verts[b + 8]];
                    let (d2, _s, _t, cp) = point_triangle(p, v0, v1, v2);
                    if d2 < best_dist_sq {
                        best_dist_sq = d2;
                        best_point = cp;
                    }
                }
            } else {
                let left = node.offset as usize;
                stack.push(left);
                stack.push(left + 1);
            }
        }

        (best_point, best_dist_sq.sqrt())
    }
}

fn aabb_dist_sq(p: [f32; 3], amin: [f32; 3], amax: [f32; 3]) -> f32 {
    let mut d2 = 0.0f32;
    for a in 0..3 {
        if p[a] < amin[a] {
            let d = amin[a] - p[a];
            d2 += d * d;
        } else if p[a] > amax[a] {
            let d = p[a] - amax[a];
            d2 += d * d;
        }
    }
    d2
}
