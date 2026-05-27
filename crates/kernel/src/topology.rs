//! Per-body B-Rep topology extracted from mesh classifications.
//!
//! Walks the DC mesh, groups Face-classified triangles by contributor
//! into connected face regions, and detects Edge-classified boundary
//! segments between different contributors. Edge segments are chained
//! into ordered polylines split at Vertex-classified points (3+ faces
//! meeting).

use std::collections::{HashMap, HashSet};

use crate::csg::MAX_CONTRIBUTORS;
use crate::dc::{connected_components, Mesh, CONTRIBUTOR_SENTINEL};

/// One topological face — a connected region of mesh triangles that
/// share the same dominant contributor.
#[derive(Debug, Clone, serde::Serialize)]
pub struct TopoFace {
    pub index: u32,
    pub contributor: u32,
    pub triangles: Vec<u32>,
    pub area: f32,
    pub centroid: [f32; 3],
    pub normal: [f32; 3],
}

/// One topological edge — an ordered polyline along the boundary
/// between two contributor regions.
#[derive(Debug, Clone, serde::Serialize)]
pub struct TopoEdge {
    pub index: u32,
    pub contributor_a: u32,
    pub contributor_b: u32,
    pub points: Vec<f32>,
}

/// Aggregate topology for one body.
#[derive(Debug, Clone, serde::Serialize)]
pub struct BodyTopology {
    pub faces: Vec<TopoFace>,
    pub edges: Vec<TopoEdge>,
}

pub fn extract_topology(mesh: &Mesh) -> BodyTopology {
    let faces = extract_faces(mesh);
    let edges = extract_edges(mesh);
    BodyTopology { faces, edges }
}

fn primary_contributor(mesh: &Mesh, vertex: usize) -> Option<u32> {
    let c = mesh.contributors[vertex * MAX_CONTRIBUTORS];
    if c == CONTRIBUTOR_SENTINEL {
        None
    } else {
        Some(c)
    }
}

fn extract_faces(mesh: &Mesh) -> Vec<TopoFace> {
    let tc = mesh.triangle_count();
    if tc == 0 {
        return vec![];
    }

    let mut tri_by_contributor: HashMap<u32, Vec<usize>> = HashMap::new();

    for t in 0..tc {
        let [i0, i1, i2] = [
            mesh.indices[t * 3] as usize,
            mesh.indices[t * 3 + 1] as usize,
            mesh.indices[t * 3 + 2] as usize,
        ];

        if mesh.classifications[i0] != 1
            || mesh.classifications[i1] != 1
            || mesh.classifications[i2] != 1
        {
            continue;
        }

        let c0 = mesh.contributors[i0 * MAX_CONTRIBUTORS];
        let c1 = mesh.contributors[i1 * MAX_CONTRIBUTORS];
        let c2 = mesh.contributors[i2 * MAX_CONTRIBUTORS];
        if c0 != c1 || c1 != c2 || c0 == CONTRIBUTOR_SENTINEL {
            continue;
        }

        tri_by_contributor.entry(c0).or_default().push(t);
    }

    let mut faces = Vec::new();
    let mut face_index = 0u32;

    let mut sorted_contributors: Vec<u32> = tri_by_contributor.keys().copied().collect();
    sorted_contributors.sort_unstable();

    for contributor in sorted_contributors {
        let tris = &tri_by_contributor[&contributor];
        let components = connected_components(tris, mesh);
        for comp in components {
            if comp.is_empty() {
                continue;
            }
            let (area, centroid, normal) = compute_face_geometry(mesh, &comp);
            faces.push(TopoFace {
                index: face_index,
                contributor,
                triangles: comp.iter().map(|&t| t as u32).collect(),
                area,
                centroid,
                normal,
            });
            face_index += 1;
        }
    }

    faces
}

fn compute_face_geometry(mesh: &Mesh, tris: &[usize]) -> (f32, [f32; 3], [f32; 3]) {
    let mut total_area = 0.0f32;
    let mut cx = 0.0f32;
    let mut cy = 0.0f32;
    let mut cz = 0.0f32;
    let mut nx = 0.0f32;
    let mut ny = 0.0f32;
    let mut nz = 0.0f32;

    for &t in tris {
        let i0 = mesh.indices[t * 3] as usize;
        let i1 = mesh.indices[t * 3 + 1] as usize;
        let i2 = mesh.indices[t * 3 + 2] as usize;

        let p0 = [
            mesh.positions[i0 * 3],
            mesh.positions[i0 * 3 + 1],
            mesh.positions[i0 * 3 + 2],
        ];
        let p1 = [
            mesh.positions[i1 * 3],
            mesh.positions[i1 * 3 + 1],
            mesh.positions[i1 * 3 + 2],
        ];
        let p2 = [
            mesh.positions[i2 * 3],
            mesh.positions[i2 * 3 + 1],
            mesh.positions[i2 * 3 + 2],
        ];

        let e1 = [p1[0] - p0[0], p1[1] - p0[1], p1[2] - p0[2]];
        let e2 = [p2[0] - p0[0], p2[1] - p0[1], p2[2] - p0[2]];
        let cross = [
            e1[1] * e2[2] - e1[2] * e2[1],
            e1[2] * e2[0] - e1[0] * e2[2],
            e1[0] * e2[1] - e1[1] * e2[0],
        ];
        let twice_area = (cross[0] * cross[0] + cross[1] * cross[1] + cross[2] * cross[2]).sqrt();
        let tri_area = twice_area * 0.5;

        let tri_cx = (p0[0] + p1[0] + p2[0]) / 3.0;
        let tri_cy = (p0[1] + p1[1] + p2[1]) / 3.0;
        let tri_cz = (p0[2] + p1[2] + p2[2]) / 3.0;

        total_area += tri_area;
        cx += tri_area * tri_cx;
        cy += tri_area * tri_cy;
        cz += tri_area * tri_cz;
        nx += cross[0];
        ny += cross[1];
        nz += cross[2];
    }

    let centroid = if total_area > 1e-12 {
        [cx / total_area, cy / total_area, cz / total_area]
    } else {
        [0.0, 0.0, 0.0]
    };

    let nlen = (nx * nx + ny * ny + nz * nz).sqrt();
    let normal = if nlen > 1e-12 {
        [nx / nlen, ny / nlen, nz / nlen]
    } else {
        [0.0, 1.0, 0.0]
    };

    (total_area, centroid, normal)
}

fn extract_edges(mesh: &Mesh) -> Vec<TopoEdge> {
    let tc = mesh.triangle_count();
    if tc == 0 {
        return vec![];
    }

    let mut seen: HashSet<(usize, usize)> = HashSet::new();
    // (contributor_a, contributor_b) -> list of (vertex_a, vertex_b)
    let mut boundary_edges: HashMap<(u32, u32), Vec<(usize, usize)>> = HashMap::new();

    for t in 0..tc {
        let tri = [
            mesh.indices[t * 3] as usize,
            mesh.indices[t * 3 + 1] as usize,
            mesh.indices[t * 3 + 2] as usize,
        ];

        for &(a, b) in &[(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])] {
            let edge_key = if a < b { (a, b) } else { (b, a) };
            if !seen.insert(edge_key) {
                continue;
            }

            let ca = primary_contributor(mesh, a);
            let cb = primary_contributor(mesh, b);

            match (ca, cb) {
                (Some(fa), Some(fb)) if fa != fb => {
                    let pair = if fa < fb { (fa, fb) } else { (fb, fa) };
                    boundary_edges.entry(pair).or_default().push((a, b));
                }
                _ => {}
            }
        }
    }

    let mut edges = Vec::new();
    let mut edge_index = 0u32;

    let mut sorted_pairs: Vec<(u32, u32)> = boundary_edges.keys().copied().collect();
    sorted_pairs.sort_unstable();

    for pair in sorted_pairs {
        let segments = &boundary_edges[&pair];
        let chains = chain_segments(segments, mesh);
        for chain in chains {
            if chain.len() < 2 {
                continue;
            }
            let mut points = Vec::with_capacity(chain.len() * 3);
            for &vi in &chain {
                points.push(mesh.positions[vi * 3]);
                points.push(mesh.positions[vi * 3 + 1]);
                points.push(mesh.positions[vi * 3 + 2]);
            }
            edges.push(TopoEdge {
                index: edge_index,
                contributor_a: pair.0,
                contributor_b: pair.1,
                points,
            });
            edge_index += 1;
        }
    }

    edges
}

fn chain_segments(segments: &[(usize, usize)], mesh: &Mesh) -> Vec<Vec<usize>> {
    if segments.is_empty() {
        return vec![];
    }

    let mut adj: HashMap<usize, Vec<usize>> = HashMap::new();
    for &(a, b) in segments {
        adj.entry(a).or_default().push(b);
        adj.entry(b).or_default().push(a);
    }

    let mut visited_edges: HashSet<(usize, usize)> = HashSet::new();
    let mut chains: Vec<Vec<usize>> = Vec::new();

    let mut starts: Vec<usize> = adj
        .iter()
        .filter(|(_, neighbors)| neighbors.len() == 1)
        .map(|(&v, _)| v)
        .collect();
    starts.sort_unstable();

    // Also consider Vertex-classified points (classification == 3) as
    // chain split points — they should start their own chains.
    let mut vertex_splits: Vec<usize> = adj
        .keys()
        .filter(|&&v| mesh.classifications[v] == 3)
        .copied()
        .collect();
    vertex_splits.sort_unstable();
    for v in &vertex_splits {
        if !starts.contains(v) {
            starts.push(*v);
        }
    }

    for &start in &starts {
        walk_chain(start, &adj, mesh, &mut visited_edges, &mut chains);
    }

    // Remaining unvisited edges form closed loops.
    let mut remaining: Vec<usize> = adj
        .keys()
        .filter(|&&v| {
            adj[&v]
                .iter()
                .any(|&n| {
                    let ek = if v < n { (v, n) } else { (n, v) };
                    !visited_edges.contains(&ek)
                })
        })
        .copied()
        .collect();
    remaining.sort_unstable();

    for start in remaining {
        walk_chain(start, &adj, mesh, &mut visited_edges, &mut chains);
    }

    chains
}

fn walk_chain(
    start: usize,
    adj: &HashMap<usize, Vec<usize>>,
    mesh: &Mesh,
    visited_edges: &mut HashSet<(usize, usize)>,
    chains: &mut Vec<Vec<usize>>,
) {
    let mut chain = vec![start];
    let mut current = start;

    loop {
        let neighbors = match adj.get(&current) {
            Some(n) => n,
            None => break,
        };
        let next = neighbors.iter().find(|&&n| {
            let ek = if current < n { (current, n) } else { (n, current) };
            !visited_edges.contains(&ek)
        });

        match next {
            Some(&n) => {
                let ek = if current < n { (current, n) } else { (n, current) };
                visited_edges.insert(ek);
                chain.push(n);

                if n == start {
                    break;
                }
                if mesh.classifications[n] == 3 {
                    if chain.len() >= 2 {
                        chains.push(chain);
                    }
                    chain = vec![n];
                }
                current = n;
            }
            None => break,
        }
    }

    if chain.len() >= 2 {
        chains.push(chain);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bvh::BoundedCsg;
    use crate::csg::CsgNode;
    use crate::dc;
    use fielddraft_protocol::{BoolOp, Feature, FeatureId, Primitive, Transform};

    fn box_feature(id: u32) -> Feature {
        Feature {
            id: FeatureId::from_u32(id),
            name: "Box".into(),
            op: BoolOp::Union,
            primitive: Primitive::Box {
                center: [0.0; 3],
                half_extents: [20.0, 20.0, 20.0],
            },
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

    fn sphere_diff(id: u32, center: [f32; 3], radius: f32) -> Feature {
        Feature {
            id: FeatureId::from_u32(id),
            name: "Sphere".into(),
            op: BoolOp::Difference,
            primitive: Primitive::Sphere { center, radius },
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

    fn mesh_for(features: &[Feature], res: u32) -> (Mesh, BoundedCsg) {
        let csg = CsgNode::from_features(features);
        let bounded = BoundedCsg::from_csg(&csg);
        let mesh = dc::dual_contour(&bounded, res);
        (mesh, bounded)
    }

    #[test]
    fn single_box_topology() {
        let features = vec![box_feature(1)];
        let (mesh, _) = mesh_for(&features, 24);
        if mesh.is_empty() {
            return;
        }
        let topo = extract_topology(&mesh);
        assert!(
            !topo.faces.is_empty(),
            "single box should produce at least one face"
        );
        assert!(
            topo.edges.is_empty(),
            "single contributor body should have no topo edges"
        );
        for face in &topo.faces {
            assert!(!face.triangles.is_empty());
            assert!(face.area > 0.0);
        }
    }

    #[test]
    fn box_minus_sphere_has_faces_and_edges() {
        let features = vec![
            box_feature(1),
            sphere_diff(2, [10.0, 0.0, 0.0], 15.0),
        ];
        let (mesh, _) = mesh_for(&features, 24);
        if mesh.is_empty() {
            return;
        }
        let topo = extract_topology(&mesh);
        assert!(
            topo.faces.len() >= 2,
            "box-minus-sphere should produce at least 2 face regions"
        );
        assert!(
            !topo.edges.is_empty(),
            "box-minus-sphere should produce topo edges at the intersection"
        );
    }

    #[test]
    fn empty_mesh_produces_empty_topology() {
        let mesh = Mesh::default();
        let topo = extract_topology(&mesh);
        assert!(topo.faces.is_empty());
        assert!(topo.edges.is_empty());
    }

    #[test]
    fn face_normals_are_unit_length() {
        let features = vec![box_feature(1)];
        let (mesh, _) = mesh_for(&features, 24);
        if mesh.is_empty() {
            return;
        }
        let topo = extract_topology(&mesh);
        for face in &topo.faces {
            let len = (face.normal[0].powi(2)
                + face.normal[1].powi(2)
                + face.normal[2].powi(2))
            .sqrt();
            assert!(
                (len - 1.0).abs() < 1e-4,
                "face normal should be unit-length, got {len}"
            );
        }
    }

    #[test]
    fn hierarchical_child_id_gets_contributor() {
        let features = vec![
            box_feature(1),
            Feature {
                id: FeatureId::from_u32(2).child("0"),
                name: "Hole".into(),
                op: BoolOp::Difference,
                primitive: Primitive::Cylinder {
                    center: [0.0, 0.0, 0.0],
                    radius: 10.0,
                    half_height: 30.0,
                },
                transform: Transform::IDENTITY,
                blend_radius: 0.0,
                chamfer_size: 0.0,
                twist_rate: 0.0,
                bend_rate: 0.0,
                displace_depth: 0.0,
                displace_freq: 0.0,
                shell_thickness: 0.0,
                target_ids: vec![],
            },
        ];
        let (mesh, _) = mesh_for(&features, 24);
        if mesh.is_empty() {
            return;
        }
        let mut has_contributor_2 = false;
        for i in 0..mesh.positions.len() / 3 {
            let c = mesh.contributors[i * crate::csg::MAX_CONTRIBUTORS];
            if c == 2 {
                has_contributor_2 = true;
                break;
            }
        }
        assert!(
            has_contributor_2,
            "hole cylinder with child id 2.0 should produce contributor 2 via root ancestor"
        );
        let topo = extract_topology(&mesh);
        assert!(
            !topo.edges.is_empty(),
            "box-minus-hole should produce topo edges"
        );
    }

    #[test]
    fn box_minus_sphere_has_edges_mdc() {
        let features = vec![
            box_feature(1),
            sphere_diff(2, [10.0, 0.0, 0.0], 15.0),
        ];
        let (mesh, _) = mesh_for(&features, 80);
        if mesh.is_empty() {
            return;
        }
        let topo = extract_topology(&mesh);
        let mut has_c2 = false;
        for i in 0..mesh.positions.len() / 3 {
            if mesh.contributors[i * crate::csg::MAX_CONTRIBUTORS] == 2 {
                has_c2 = true;
                break;
            }
        }
        assert!(has_c2, "MDC path should produce contributor 2");
        assert!(
            !topo.edges.is_empty(),
            "box-minus-sphere at MDC resolution should produce topo edges, faces={} edges={}",
            topo.faces.len(),
            topo.edges.len()
        );
    }

    #[test]
    fn edge_contributor_pairs_are_canonical() {
        let features = vec![
            box_feature(1),
            sphere_diff(2, [10.0, 0.0, 0.0], 15.0),
        ];
        let (mesh, _) = mesh_for(&features, 24);
        if mesh.is_empty() {
            return;
        }
        let topo = extract_topology(&mesh);
        for edge in &topo.edges {
            assert!(
                edge.contributor_a <= edge.contributor_b,
                "edge contributors must be canonical (a <= b)"
            );
            assert!(
                edge.points.len() >= 6,
                "each edge polyline needs at least 2 points (6 floats)"
            );
        }
    }
}
