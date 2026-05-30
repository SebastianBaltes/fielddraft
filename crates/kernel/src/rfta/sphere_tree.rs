//! SDF spheres and spatial queries via AABB tree.

pub struct SdfSphere {
    pub center: [f32; 3],
    pub radius: f32,
    pub positive: bool,
}

impl SdfSphere {
    /// Signed radius: positive for outside spheres, negative for inside.
    pub fn signed_radius(&self) -> f32 {
        if self.positive {
            self.radius
        } else {
            -self.radius
        }
    }
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

    /// Returns true if `p` lies inside any sphere whose radius has been
    /// scaled by `shrink` (e.g. 0.95 for the rasterization raster-shrink).
    /// Sign is ignored — callers pass a single-sign subset. Early-exits on
    /// the first containing sphere, so interior points resolve in O(depth).
    ///
    /// The node AABBs are built from the *full* radii, so a point inside a
    /// shrunk sphere is always inside the enclosing AABB — pruning stays
    /// valid even though the effective spheres are smaller.
    pub fn point_inside_any_shrunk(
        &self,
        spheres: &[SdfSphere],
        p: [f32; 3],
        shrink: f32,
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
                    let s = &spheres[self.order[i] as usize];
                    let sr = s.radius * shrink;
                    let dx = p[0] - s.center[0];
                    let dy = p[1] - s.center[1];
                    let dz = p[2] - s.center[2];
                    if dx * dx + dy * dy + dz * dz < sr * sr {
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

    /// Returns the k closest spheres (by center distance) of the given sign
    /// to `point`, excluding `exclude_idx`. Returns sphere indices sorted by
    /// distance (nearest first).
    pub fn k_closest_spheres(
        &self,
        spheres: &[SdfSphere],
        point: [f32; 3],
        k: usize,
        positive: bool,
        exclude_idx: usize,
    ) -> Vec<usize> {
        if self.nodes.is_empty() || k == 0 {
            return vec![];
        }
        // Use a max-heap of (distance_sq, index) with capacity k.
        // We maintain at most k entries; the heap top is the *farthest*
        // among the k closest found so far.
        let mut heap: Vec<(f32, usize)> = Vec::with_capacity(k + 1);
        let mut best_dist_sq = f32::INFINITY;

        let mut stack = vec![0usize];
        while let Some(ni) = stack.pop() {
            let node = &self.nodes[ni];
            let box_dist = aabb_point_dist_sq(point, node.aabb_min, node.aabb_max);
            if box_dist > best_dist_sq {
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
                    let dx = point[0] - s.center[0];
                    let dy = point[1] - s.center[1];
                    let dz = point[2] - s.center[2];
                    let d2 = dx * dx + dy * dy + dz * dz;
                    if d2 < best_dist_sq || heap.len() < k {
                        // Insert into max-heap
                        heap.push((d2, si));
                        // Bubble up
                        let mut ci = heap.len() - 1;
                        while ci > 0 {
                            let pi = (ci - 1) / 2;
                            if heap[ci].0 > heap[pi].0 {
                                heap.swap(ci, pi);
                                ci = pi;
                            } else {
                                break;
                            }
                        }
                        if heap.len() > k {
                            // Remove max (root)
                            heap[0] = heap[heap.len() - 1];
                            heap.pop();
                            // Sift down
                            sift_down_max(&mut heap);
                        }
                        best_dist_sq = if heap.len() == k {
                            heap[0].0
                        } else {
                            f32::INFINITY
                        };
                    }
                }
            } else {
                let left = node.offset as usize;
                stack.push(left);
                stack.push(left + 1);
            }
        }

        // Sort by distance ascending
        heap.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        heap.iter().map(|&(_, idx)| idx).collect()
    }

    /// Returns indices of all spheres of the given sign that contain `point`
    /// (i.e. distance from center < radius + tol), excluding `exclude_idx`.
    pub fn get_spheres_containing(
        &self,
        spheres: &[SdfSphere],
        point: [f32; 3],
        positive: bool,
        exclude_idx: usize,
        tol: f32,
    ) -> Vec<usize> {
        if self.nodes.is_empty() {
            return vec![];
        }
        let mut result = Vec::new();
        let mut stack = vec![0usize];
        while let Some(ni) = stack.pop() {
            let node = &self.nodes[ni];
            // Check if point could be inside any sphere in this node's AABB
            // expanded by the maximum sphere radius + tol. The AABB already
            // includes sphere radii from the build step, so we just need tol.
            if point[0] < node.aabb_min[0] - tol
                || point[0] > node.aabb_max[0] + tol
                || point[1] < node.aabb_min[1] - tol
                || point[1] > node.aabb_max[1] + tol
                || point[2] < node.aabb_min[2] - tol
                || point[2] > node.aabb_max[2] + tol
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
                    let dx = point[0] - s.center[0];
                    let dy = point[1] - s.center[1];
                    let dz = point[2] - s.center[2];
                    let d2 = dx * dx + dy * dy + dz * dz;
                    let r_tol = s.radius + tol;
                    if d2 < r_tol * r_tol {
                        result.push(si);
                    }
                }
            } else {
                let left = node.offset as usize;
                stack.push(left);
                stack.push(left + 1);
            }
        }
        result
    }
}

fn aabb_point_dist_sq(p: [f32; 3], amin: [f32; 3], amax: [f32; 3]) -> f32 {
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

fn sift_down_max(heap: &mut [(f32, usize)]) {
    let n = heap.len();
    let mut i = 0;
    loop {
        let left = 2 * i + 1;
        let right = 2 * i + 2;
        let mut largest = i;
        if left < n && heap[left].0 > heap[largest].0 {
            largest = left;
        }
        if right < n && heap[right].0 > heap[largest].0 {
            largest = right;
        }
        if largest == i {
            break;
        }
        heap.swap(i, largest);
        i = largest;
    }
}
