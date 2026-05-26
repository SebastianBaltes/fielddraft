//! Adaptive Dual Contouring with QEF vertex placement (K3.7).
//!
//! Replaces the uniform/sparse DC paths for resolutions above 64.
//! Uses a recursive octree that subdivides only where the SDF has a
//! sign change, and places vertices via QEF (Quadratic Error Function)
//! instead of cell centroids — producing sharp edges on boxes and
//! adaptive resolution on curved surfaces.

use crate::bvh::BoundedCsg;
use crate::csg::{EvalMode, ProxyElement, MAX_CONTRIBUTORS};
use crate::dc::{sdf_normal, Mesh, CONTRIBUTOR_SENTINEL};

// ── Lookup tables ────────────────────────────────────────────────────

/// Edge endpoint corner indices (12 edges of a cube).
const EDGE_CORNERS: [[usize; 2]; 12] = [
    [0, 1], [2, 3], [4, 5], [6, 7], // X-axis edges
    [0, 2], [1, 3], [4, 6], [5, 7], // Y-axis edges
    [0, 4], [1, 5], [2, 6], [3, 7], // Z-axis edges
];

/// Corner offsets in [x, y, z] order. Corner 0 = min corner.
const CORNER_OFFSETS: [[f32; 3]; 8] = [
    [0.0, 0.0, 0.0], [1.0, 0.0, 0.0],
    [0.0, 1.0, 0.0], [1.0, 1.0, 0.0],
    [0.0, 0.0, 1.0], [1.0, 0.0, 1.0],
    [0.0, 1.0, 1.0], [1.0, 1.0, 1.0],
];

/// Which cells share each of the 12 edges of a parent cell when
/// processing faces/edges. ProcessFace examines pairs of children
/// sharing a face; ProcessEdge examines quads of children sharing
/// an edge.
///
/// CellProcFaceMask: for each of the 3 axes, 4 face-sharing pairs.
/// [axis][pair] => [child0, child1]
const CELL_PROC_FACE_MASK: [[[usize; 2]; 4]; 3] = [
    [[0, 1], [2, 3], [4, 5], [6, 7]], // X faces
    [[0, 2], [1, 3], [4, 6], [5, 7]], // Y faces
    [[0, 4], [1, 5], [2, 6], [3, 7]], // Z faces
];

/// CellProcEdgeMask: for each of the 3 edge axes, 2 quads of children.
/// [axis][quad] => [child0, child1, child2, child3]
const CELL_PROC_EDGE_MASK: [[[usize; 4]; 2]; 3] = [
    [[0, 2, 4, 6], [1, 3, 5, 7]], // X-axis edges
    [[0, 1, 4, 5], [2, 3, 6, 7]], // Y-axis edges
    [[0, 1, 2, 3], [4, 5, 6, 7]], // Z-axis edges
];

/// FaceProcEdgeMask: for each face orientation, which edges to process.
/// Maps face-sharing pairs into edge-sharing quads.
/// [face_axis][sub] => [child0, child1, child2, child3, edge_axis]
const FACE_PROC_EDGE_MASK: [[[usize; 5]; 4]; 3] = [
    // X faces → Y and Z edges
    [[0, 1, 2, 3, 1], [4, 5, 6, 7, 1], [0, 1, 4, 5, 2], [2, 3, 6, 7, 2]],
    // Y faces → X and Z edges
    [[0, 2, 1, 3, 0], [4, 6, 5, 7, 0], [0, 2, 4, 6, 2], [1, 3, 5, 7, 2]],
    // Z faces → X and Y edges
    [[0, 4, 1, 5, 0], [2, 6, 3, 7, 0], [0, 4, 2, 6, 1], [1, 5, 3, 7, 1]],
];

/// For ProcessEdge: which two corners of the 4 cells form the actual
/// edge being processed. [edge_axis] => [corner_in_cell_a, corner_in_cell_d]
const PROCESS_EDGE_MASK: [[usize; 2]; 3] = [
    [0, 7], // X edge: corners 0 and 7
    [0, 7], // Y edge: corners 0 and 7
    [0, 7], // Z edge: corners 0 and 7
];

// ── QEF Solver ───────────────────────────────────────────────────────

struct QefData {
    ata: [f64; 6], // upper triangle of A^T A (symmetric 3x3)
    atb: [f64; 3], // A^T b
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

        // A^T A accumulation (upper triangle, row-major)
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

        // Solve (A^T A) x = A^T b via pseudoinverse using Givens rotations.
        // For robustness, solve in mass-point-centered coordinates, then
        // translate back. The matrix is only 3x3 so direct SVD is fine.
        let solved = svd_solve_3x3(&self.ata, &self.atb, &mp);
        [solved[0] as f32, solved[1] as f32, solved[2] as f32]
    }
}

fn svd_solve_3x3(ata: &[f64; 6], atb: &[f64; 3], mass_point: &[f64; 3]) -> [f64; 3] {
    // Expand upper triangle to full symmetric matrix
    let mut a = [
        [ata[0], ata[1], ata[2]],
        [ata[1], ata[3], ata[4]],
        [ata[2], ata[4], ata[5]],
    ];

    // Shift to mass-point coordinates: b' = b - A * mp
    let b = [
        atb[0] - (a[0][0] * mass_point[0] + a[0][1] * mass_point[1] + a[0][2] * mass_point[2]),
        atb[1] - (a[1][0] * mass_point[0] + a[1][1] * mass_point[1] + a[1][2] * mass_point[2]),
        atb[2] - (a[2][0] * mass_point[0] + a[2][1] * mass_point[1] + a[2][2] * mass_point[2]),
    ];

    // Jacobi SVD: diagonalize A^T A via sweeps
    let mut v = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    for _ in 0..10 {
        for p in 0..3 {
            for q in (p + 1)..3 {
                jacobi_rotation(&mut a, &mut v, p, q);
            }
        }
    }

    // Eigenvalues are the diagonal of `a`, eigenvectors are columns of `v`.
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

    // Rotate a
    let app = a[p][p];
    let aqq = a[q][q];
    let apq = a[p][q];
    a[p][p] = c * c * app - 2.0 * s * c * apq + s * s * aqq;
    a[q][q] = s * s * app + 2.0 * s * c * apq + c * c * aqq;
    a[p][q] = 0.0;
    a[q][p] = 0.0;
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

    // Rotate v
    for r in 0..3 {
        let vrp = v[r][p];
        let vrq = v[r][q];
        v[r][p] = c * vrp - s * vrq;
        v[r][q] = s * vrp + c * vrq;
    }
}

// ── Octree ───────────────────────────────────────────────────────────

struct OctreeNode {
    min: [f32; 3],
    size: f32,
    kind: OctreeNodeKind,
}

enum OctreeNodeKind {
    Internal { children: [Option<Box<OctreeNode>>; 8] },
    Leaf { corners: [f32; 8], vertex_index: Option<usize> },
}

struct MdcVertex {
    pos: [f32; 3],
    normal: [f32; 3],
    classification: u8,
    contributors: [u32; MAX_CONTRIBUTORS],
}

fn corner_pos(min: [f32; 3], size: f32, corner: usize) -> [f32; 3] {
    [
        min[0] + CORNER_OFFSETS[corner][0] * size,
        min[1] + CORNER_OFFSETS[corner][1] * size,
        min[2] + CORNER_OFFSETS[corner][2] * size,
    ]
}

fn has_sign_change(corners: &[f32; 8]) -> bool {
    let first_sign = corners[0] > 0.0;
    corners[1..].iter().any(|&c| (c > 0.0) != first_sign)
}

fn cell_overlaps_model(node: &BoundedCsg, min: [f32; 3], size: f32) -> bool {
    let aabb = node.aabb();
    if aabb.is_empty() {
        return false;
    }
    let max = [min[0] + size, min[1] + size, min[2] + size];
    min[0] <= aabb.max[0] && max[0] >= aabb.min[0]
        && min[1] <= aabb.max[1] && max[1] >= aabb.min[1]
        && min[2] <= aabb.max[2] && max[2] >= aabb.min[2]
}

fn build_octree(
    node: &BoundedCsg,
    min: [f32; 3],
    size: f32,
    max_depth: u32,
    depth: u32,
) -> Option<OctreeNode> {
    if !cell_overlaps_model(node, min, size) {
        return None;
    }

    let corners: [f32; 8] = std::array::from_fn(|i| {
        let p = corner_pos(min, size, i);
        node.sdf(p, EvalMode::Visual)
    });

    if depth >= max_depth {
        if !has_sign_change(&corners) {
            return None;
        }
        return Some(OctreeNode {
            min,
            size,
            kind: OctreeNodeKind::Leaf {
                corners,
                vertex_index: None,
            },
        });
    }

    // At non-leaf levels, recurse into children even if this cell's
    // corners don't show a sign change — the surface may exist
    // entirely between corners.
    let half = size * 0.5;
    let children: [Option<Box<OctreeNode>>; 8] = std::array::from_fn(|i| {
        let child_min = [
            min[0] + CORNER_OFFSETS[i][0] * half,
            min[1] + CORNER_OFFSETS[i][1] * half,
            min[2] + CORNER_OFFSETS[i][2] * half,
        ];
        build_octree(node, child_min, half, max_depth, depth + 1)
            .map(Box::new)
    });

    let any_child = children.iter().any(|c| c.is_some());
    if !any_child {
        return None;
    }

    Some(OctreeNode {
        min,
        size,
        kind: OctreeNodeKind::Internal { children },
    })
}

// ── Vertex placement ─────────────────────────────────────────────────

fn place_vertices(
    octree: &mut OctreeNode,
    node: &BoundedCsg,
    vertices: &mut Vec<MdcVertex>,
    normal_eps: f32,
) {
    match &mut octree.kind {
        OctreeNodeKind::Internal { children } => {
            for child in children.iter_mut().flatten() {
                place_vertices(child, node, vertices, normal_eps);
            }
        }
        OctreeNodeKind::Leaf { corners, vertex_index } => {
            let mut qef = QefData::new();
            let min = octree.min;
            let size = octree.size;

            for &[c0, c1] in &EDGE_CORNERS {
                let v0 = corners[c0];
                let v1 = corners[c1];
                if (v0 > 0.0) == (v1 > 0.0) {
                    continue;
                }
                let t = v0 / (v0 - v1);
                let p0 = corner_pos(min, size, c0);
                let p1 = corner_pos(min, size, c1);
                let p = [
                    p0[0] + t * (p1[0] - p0[0]),
                    p0[1] + t * (p1[1] - p0[1]),
                    p0[2] + t * (p1[2] - p0[2]),
                ];
                let n = sdf_normal(node, p, normal_eps);
                qef.add(p, n);
            }

            let mut pos = qef.solve();

            // Clamp to cell bounds
            pos[0] = pos[0].clamp(min[0], min[0] + size);
            pos[1] = pos[1].clamp(min[1], min[1] + size);
            pos[2] = pos[2].clamp(min[2], min[2] + size);

            let normal = sdf_normal(node, pos, normal_eps);

            // Evaluate contributors
            let (_, cs) = node.evaluate(pos, normal_eps, EvalMode::Visual);
            let mut element = cs.element();
            if element == ProxyElement::None {
                element = ProxyElement::Face;
            }
            let slice = cs.as_slice();
            let mut contributors = [CONTRIBUTOR_SENTINEL; MAX_CONTRIBUTORS];
            for slot in 0..MAX_CONTRIBUTORS {
                if slot < slice.len() {
                    contributors[slot] = slice[slot]
                        .as_u32_if_flat()
                        .unwrap_or(CONTRIBUTOR_SENTINEL);
                }
            }

            let idx = vertices.len();
            vertices.push(MdcVertex {
                pos,
                normal,
                classification: element.tag(),
                contributors,
            });
            *vertex_index = Some(idx);
        }
    }
}

// ── Quad emission (ProcessCell / ProcessFace / ProcessEdge) ──────────

fn process_cell(octree: &OctreeNode, mesh: &mut MeshBuilder) {
    if let OctreeNodeKind::Internal { children } = &octree.kind {
        // Recurse into children
        for child in children.iter().flatten() {
            process_cell(child, mesh);
        }

        // Process shared faces between child pairs
        for axis in 0..3 {
            for pair in &CELL_PROC_FACE_MASK[axis] {
                let (c0, c1) = (pair[0], pair[1]);
                if let (Some(a), Some(b)) = (&children[c0], &children[c1]) {
                    process_face(a, b, axis, mesh);
                }
            }
        }

        // Process shared edges between child quads
        for axis in 0..3 {
            for quad in &CELL_PROC_EDGE_MASK[axis] {
                let nodes: [Option<&OctreeNode>; 4] = [
                    children[quad[0]].as_deref(),
                    children[quad[1]].as_deref(),
                    children[quad[2]].as_deref(),
                    children[quad[3]].as_deref(),
                ];
                if nodes.iter().any(|n| n.is_some()) {
                    process_edge(&nodes, axis, mesh);
                }
            }
        }
    }
}

fn process_face(
    a: &OctreeNode,
    b: &OctreeNode,
    axis: usize,
    mesh: &mut MeshBuilder,
) {
    let a_internal = matches!(&a.kind, OctreeNodeKind::Internal { .. });
    let b_internal = matches!(&b.kind, OctreeNodeKind::Internal { .. });

    if !a_internal && !b_internal {
        // Both leaves — nothing to emit at the face level.
        // Edge processing will handle the quads.
        return;
    }

    // Process edges that cross this face
    for sub in &FACE_PROC_EDGE_MASK[axis] {
        let edge_axis = sub[4];
        let nodes: [Option<&OctreeNode>; 4] = std::array::from_fn(|i| {
            let child_idx = sub[i];
            // Determine which parent (a or b) owns this child
            // and descend into it if internal.
            let (parent, local_idx) = if child_idx < 4 {
                (a, child_idx)
            } else {
                (b, child_idx - 4)
            };
            match &parent.kind {
                OctreeNodeKind::Internal { children } => children[local_idx].as_deref(),
                OctreeNodeKind::Leaf { .. } => Some(parent),
            }
        });
        if nodes.iter().any(|n| n.is_some()) {
            process_edge(&nodes, edge_axis, mesh);
        }
    }
}

fn process_edge(
    nodes: &[Option<&OctreeNode>; 4],
    axis: usize,
    mesh: &mut MeshBuilder,
) {
    // If any node is internal, we need to recurse deeper.
    let any_internal = nodes.iter().any(|n| {
        n.map_or(false, |n| matches!(&n.kind, OctreeNodeKind::Internal { .. }))
    });

    if any_internal {
        // Recurse: each internal node contributes its relevant child;
        // leaf nodes repeat themselves for all sub-edges.
        for sub in &CELL_PROC_EDGE_MASK[axis] {
            let sub_nodes: [Option<&OctreeNode>; 4] = std::array::from_fn(|i| {
                let parent = nodes[i]?;
                match &parent.kind {
                    OctreeNodeKind::Internal { children } => children[sub[i]].as_deref(),
                    OctreeNodeKind::Leaf { .. } => Some(parent),
                }
            });
            if sub_nodes.iter().any(|n| n.is_some()) {
                process_edge(&sub_nodes, axis, mesh);
            }
        }
        return;
    }

    // All four are leaves (or None). Check if the edge has a sign change.
    let [c0_idx, c1_idx] = PROCESS_EDGE_MASK[axis];
    let mut v0_sign = None;
    let mut v1_sign = None;

    for n in nodes.iter().flatten() {
        if let OctreeNodeKind::Leaf { corners, .. } = &n.kind {
            if v0_sign.is_none() { v0_sign = Some(corners[c0_idx] > 0.0); }
            if v1_sign.is_none() { v1_sign = Some(corners[c1_idx] > 0.0); }
        }
    }

    let (Some(s0), Some(s1)) = (v0_sign, v1_sign) else { return };
    if s0 == s1 {
        return;
    }

    // Collect vertex indices from the four leaf cells
    let mut vi = [None; 4];
    for (i, n) in nodes.iter().enumerate() {
        if let Some(node) = n {
            if let OctreeNodeKind::Leaf { vertex_index, .. } = &node.kind {
                vi[i] = *vertex_index;
            }
        }
    }

    // Emit a quad (two triangles) from the four vertices
    if let (Some(a), Some(b), Some(c), Some(d)) = (vi[0], vi[1], vi[2], vi[3]) {
        if a == b || a == c || a == d || b == c || b == d || c == d {
            return;
        }
        // Wind based on sign: if inside is on the c0 side, flip
        if s0 {
            mesh.push_tri(a, b, c);
            mesh.push_tri(c, b, d);
        } else {
            mesh.push_tri(a, c, b);
            mesh.push_tri(b, c, d);
        }
    }
}

// ── Mesh builder ─────────────────────────────────────────────────────

struct MeshBuilder {
    indices: Vec<u32>,
}

impl MeshBuilder {
    fn new() -> Self {
        Self { indices: Vec::new() }
    }

    fn push_tri(&mut self, a: usize, b: usize, c: usize) {
        self.indices.push(a as u32);
        self.indices.push(b as u32);
        self.indices.push(c as u32);
    }
}

// ── Public entry point ───────────────────────────────────────────────

pub fn mdc_core(
    node: &BoundedCsg,
    grid_min: [f32; 3],
    grid_size: f32,
    resolution: u32,
) -> Mesh {
    let max_depth = (resolution as f32).log2().ceil() as u32;

    let mut octree = match build_octree(node, grid_min, grid_size, max_depth, 0) {
        Some(tree) => tree,
        None => return Mesh::default(),
    };

    let normal_eps = grid_size / resolution as f32 * 0.25;
    let mut vertices = Vec::new();
    place_vertices(&mut octree, node, &mut vertices, normal_eps);

    let mut builder = MeshBuilder::new();
    process_cell(&octree, &mut builder);

    // Pack into Mesh
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
        indices: builder.indices,
        classifications,
        contributors,
    }
}

// ── Tests ────────────────────────────────────────────────────────────

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

    fn bounded_sphere(center: [f32; 3], radius: f32) -> BoundedCsg {
        let f = make_feature(1, Primitive::Sphere { center, radius });
        let csg = CsgNode::from_features(&[f]);
        BoundedCsg::from_csg(&csg)
    }

    fn bounded_box(center: [f32; 3], half: [f32; 3]) -> BoundedCsg {
        let f = make_feature(1, Primitive::Box { center, half_extents: half });
        let csg = CsgNode::from_features(&[f]);
        BoundedCsg::from_csg(&csg)
    }

    #[test]
    fn octree_prunes_empty_cells() {
        let bounded = bounded_sphere([0.0, 0.0, 0.0], 5.0);
        let tree = build_octree(&bounded, [-10.0, -10.0, -10.0], 20.0, 3, 0);
        assert!(tree.is_some());

        // A sphere in a large grid should prune many cells
        fn count_leaves(node: &OctreeNode) -> usize {
            match &node.kind {
                OctreeNodeKind::Leaf { .. } => 1,
                OctreeNodeKind::Internal { children } => {
                    children.iter().flatten().map(|c| count_leaves(c)).sum()
                }
            }
        }

        let total_possible = 8usize.pow(3); // 512 at depth 3
        let actual = count_leaves(tree.as_ref().unwrap());
        assert!(
            actual < total_possible,
            "octree should prune: {actual} leaves vs {total_possible} possible"
        );
    }

    #[test]
    fn qef_places_vertex_on_edge() {
        // A plane with normal (1,0,0) through x=5 should place vertex at x≈5
        let mut qef = QefData::new();
        qef.add([5.0, 0.0, 0.0], [1.0, 0.0, 0.0]);
        qef.add([5.0, 1.0, 0.0], [1.0, 0.0, 0.0]);
        qef.add([5.0, 0.0, 1.0], [1.0, 0.0, 0.0]);
        let pos = qef.solve();
        assert!((pos[0] - 5.0).abs() < 0.1, "x should be ~5, got {}", pos[0]);
    }

    #[test]
    fn sphere_vertices_hug_surface() {
        let radius = 10.0;
        let bounded = bounded_sphere([0.0, 0.0, 0.0], radius);
        let aabb = bounded.aabb();
        let grid_size = aabb.size().iter().copied().fold(0.0f32, f32::max);
        let grid_min = aabb.min.map(|v| v - 1.0);
        let grid_total = grid_size + 2.0;

        let mesh = mdc_core(&bounded, grid_min, grid_total, 64);

        assert!(mesh.vertex_count() > 0, "mesh should have vertices");
        assert!(mesh.triangle_count() > 0, "mesh should have triangles");

        let mut max_dist = 0.0f32;
        for i in 0..mesh.vertex_count() {
            let x = mesh.positions[i * 3];
            let y = mesh.positions[i * 3 + 1];
            let z = mesh.positions[i * 3 + 2];
            let dist = (x * x + y * y + z * z).sqrt() - radius;
            max_dist = max_dist.max(dist.abs());
        }
        assert!(
            max_dist < 1.0,
            "sphere vertices should be within 1mm of surface, max dist = {max_dist}"
        );
    }

    #[test]
    fn mdc_produces_valid_mesh() {
        let bounded = bounded_box([0.0, 0.0, 0.0], [5.0, 5.0, 5.0]);
        let aabb = bounded.aabb();
        let grid_size = aabb.size().iter().copied().fold(0.0f32, f32::max);
        let grid_min = aabb.min.map(|v| v - 1.0);
        let grid_total = grid_size + 2.0;

        let mesh = mdc_core(&bounded, grid_min, grid_total, 32);

        assert!(mesh.vertex_count() > 0, "box mesh should have vertices");
        assert!(mesh.triangle_count() > 0, "box mesh should have triangles");

        // All indices should be in bounds
        for &idx in &mesh.indices {
            assert!(
                (idx as usize) < mesh.vertex_count(),
                "index {idx} out of bounds (vertex_count = {})",
                mesh.vertex_count()
            );
        }

        assert_eq!(mesh.classifications.len(), mesh.vertex_count());
        assert_eq!(mesh.contributors.len(), mesh.vertex_count() * MAX_CONTRIBUTORS);
    }

    #[test]
    fn box_preserves_sharp_edges() {
        let half = [5.0, 5.0, 5.0];
        let bounded = bounded_box([0.0, 0.0, 0.0], half);
        let aabb = bounded.aabb();
        let grid_size = aabb.size().iter().copied().fold(0.0f32, f32::max);
        let grid_min = aabb.min.map(|v| v - 1.0);
        let grid_total = grid_size + 2.0;

        let mesh = mdc_core(&bounded, grid_min, grid_total, 32);
        assert!(mesh.vertex_count() > 0);

        // Check that some vertices are near the box corners.
        // A perfect QEF solver should place vertices exactly at
        // corners/edges; we allow some tolerance.
        let corners = [
            [-5.0, -5.0, -5.0], [5.0, -5.0, -5.0],
            [-5.0, 5.0, -5.0], [5.0, 5.0, -5.0],
            [-5.0, -5.0, 5.0], [5.0, -5.0, 5.0],
            [-5.0, 5.0, 5.0], [5.0, 5.0, 5.0],
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
            if best < 2.0 {
                corners_found += 1;
            }
        }
        assert!(
            corners_found >= 6,
            "QEF should place vertices near box corners, found {corners_found}/8"
        );
    }
}
