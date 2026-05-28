//! SDF spheres and spatial queries via AABB tree.

pub struct SdfSphere {
    pub center: [f32; 3],
    pub radius: f32,
    pub positive: bool,
}

struct AabbNode {
    aabb_min: [f32; 3],
    aabb_max: [f32; 3],
    /// Interior: left child index (right = left + 1).
    /// Leaf: first sphere index in `order`.
    offset: u32,
    /// 0 = interior, >0 = sphere count (leaf).
    count: u32,
}

const LEAF_SIZE: usize = 8;

pub struct SphereAabbTree {
    nodes: Vec<AabbNode>,
    order: Vec<u32>,
}

impl SphereAabbTree {
    pub fn build(spheres: &[SdfSphere]) -> Self {
        let n = spheres.len();
        if n == 0 {
            return Self {
                nodes: vec![],
                order: vec![],
            };
        }

        let mut order: Vec<u32> = (0..n as u32).collect();
        let mut nodes: Vec<AabbNode> = Vec::with_capacity(2 * n);
        nodes.push(AabbNode {
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

        let mut tasks = vec![Task { node_idx: 0, start: 0, end: n }];

        while let Some(task) = tasks.pop() {
            let (start, end) = (task.start, task.end);
            let count = end - start;

            let mut amin = [f32::INFINITY; 3];
            let mut amax = [f32::NEG_INFINITY; 3];
            for i in start..end {
                let s = &spheres[order[i] as usize];
                for a in 0..3 {
                    amin[a] = amin[a].min(s.center[a] - s.radius);
                    amax[a] = amax[a].max(s.center[a] + s.radius);
                }
            }
            nodes[task.node_idx].aabb_min = amin;
            nodes[task.node_idx].aabb_max = amax;

            if count <= LEAF_SIZE {
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
                spheres[a as usize].center[axis]
                    .partial_cmp(&spheres[b as usize].center[axis])
                    .unwrap_or(std::cmp::Ordering::Equal)
            });

            let left_idx = nodes.len();
            nodes.push(AabbNode {
                aabb_min: [0.0; 3],
                aabb_max: [0.0; 3],
                offset: 0,
                count: 0,
            });
            nodes.push(AabbNode {
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

        Self { nodes, order }
    }

    /// Returns true if `p` is inside any sphere of the given sign except `exclude_idx`.
    pub fn point_inside_any_same_sign(
        &self,
        spheres: &[SdfSphere],
        p: [f32; 3],
        positive: bool,
        exclude_idx: usize,
    ) -> bool {
        if self.nodes.is_empty() {
            return false;
        }
        let mut stack = vec![0usize];
        while let Some(ni) = stack.pop() {
            let node = &self.nodes[ni];
            if p[0] < node.aabb_min[0] || p[0] > node.aabb_max[0]
                || p[1] < node.aabb_min[1] || p[1] > node.aabb_max[1]
                || p[2] < node.aabb_min[2] || p[2] > node.aabb_max[2]
            {
                continue;
            }
            if node.count > 0 {
                for i in node.offset as usize..(node.offset + node.count) as usize {
                    let si = self.order[i] as usize;
                    if si == exclude_idx {
                        continue;
                    }
                    let s = &spheres[si];
                    if s.positive != positive {
                        continue;
                    }
                    let dx = p[0] - s.center[0];
                    let dy = p[1] - s.center[1];
                    let dz = p[2] - s.center[2];
                    if dx * dx + dy * dy + dz * dz < s.radius * s.radius {
                        return true;
                    }
                }
            } else {
                let left = node.offset as usize;
                stack.push(left);
                stack.push(left + 1);
            }
        }
        false
    }

}
