//! Export-mesh validation (Meshing Phase 0).
//!
//! Answers the two questions every exported mesh must answer honestly
//! (docs/MESHING-EXPORT-ANALYSIS.md §1): *is it watertight* and *how far
//! is it from the true surface*. Topology is checked structurally
//! (edge-incidence over position-welded vertices); geometric deviation is
//! measured by sampling triangle interiors against the exact SDF — the
//! SDF value at a sample point IS its distance to the true surface, so
//! no reference mesh is needed.
//!
//! The checks report, they never repair — repair policy is a Phase-1+
//! decision that needs the measurements this module produces.

use crate::dc::Mesh;
use std::collections::HashMap;

/// Structural + geometric verdict for one exported mesh. Serialised
/// verbatim into the export report the UI shows.
#[derive(Debug, Clone, serde::Serialize)]
pub struct MeshValidation {
    pub vertices: usize,
    pub triangles: usize,
    /// Triangles with a repeated vertex index or (position-welded)
    /// collapsed edge — they contribute nothing but slicer warnings.
    pub degenerate_triangles: usize,
    /// Edges with exactly one incident triangle — a hole.
    pub boundary_edges: usize,
    /// Edges with more than two incident triangles.
    pub non_manifold_edges: usize,
    /// Edges whose two incident triangles wind the same way — the
    /// surface flips orientation across them.
    pub misoriented_edges: usize,
    /// Connected components (a body exported as one shell should be 1,
    /// but multiple shells are legal — e.g. a hollow part's cavity).
    pub components: usize,
    /// V − E + F over the welded mesh (2 per sphere-like shell,
    /// 2 − 2g·shells in general). Purely informative.
    pub euler_characteristic: i64,
    /// The Phase-0 headline: no boundary, no non-manifold, no
    /// misoriented edges, no degenerate triangles.
    pub watertight: bool,
    /// Largest |SDF| over all deviation samples, in mm. 0 when no
    /// samples were taken (empty mesh or passthrough body).
    pub max_deviation_mm: f32,
    /// 95th percentile of |SDF| over the samples, in mm.
    pub p95_deviation_mm: f32,
    /// Number of surface samples measured against the exact SDF.
    pub deviation_samples: usize,
}

impl MeshValidation {
    pub fn empty() -> Self {
        Self {
            vertices: 0,
            triangles: 0,
            degenerate_triangles: 0,
            boundary_edges: 0,
            non_manifold_edges: 0,
            misoriented_edges: 0,
            components: 0,
            euler_characteristic: 0,
            watertight: false,
            max_deviation_mm: 0.0,
            p95_deviation_mm: 0.0,
            deviation_samples: 0,
        }
    }
}

/// Check the mesh's topology: watertightness, manifoldness, orientation
/// consistency, component count. Vertices are welded by *exact* f32
/// position first — the meshers share vertices by index, but planar
/// simplification may leave coincident duplicates, and STL consumers
/// weld by position anyway, so position is the criterion that matters.
pub fn validate_topology(mesh: &Mesh) -> MeshValidation {
    let mut v = MeshValidation::empty();
    let vertex_count = mesh.vertex_count();
    let triangle_count = mesh.triangle_count();
    v.vertices = vertex_count;
    v.triangles = triangle_count;
    if triangle_count == 0 {
        return v;
    }

    let (weld, welded_count) = weld_by_position(mesh);

    // ── Edge incidence + orientation balance ────────────────────
    // Key: undirected welded edge. Value: (incident triangle count,
    // direction balance). Two consistently-oriented neighbours traverse
    // the shared edge in opposite directions, so their balance cancels.
    let mut edges: HashMap<(u32, u32), (u32, i32)> = HashMap::new();
    // Union-Find over welded vertices for the component count.
    let mut parent: Vec<u32> = (0..welded_count as u32).collect();
    fn find(parent: &mut [u32], mut x: u32) -> u32 {
        while parent[x as usize] != x {
            parent[x as usize] = parent[parent[x as usize] as usize];
            x = parent[x as usize];
        }
        x
    }

    let mut used = vec![false; welded_count];
    for tri in mesh.indices.chunks_exact(3) {
        let a = weld[tri[0] as usize];
        let b = weld[tri[1] as usize];
        let c = weld[tri[2] as usize];
        if a == b || b == c || c == a {
            v.degenerate_triangles += 1;
            continue;
        }
        for (s, e) in [(a, b), (b, c), (c, a)] {
            let (key, dir) = if s < e { ((s, e), 1) } else { ((e, s), -1) };
            let entry = edges.entry(key).or_insert((0, 0));
            entry.0 += 1;
            entry.1 += dir;
            let (rs, re) = (find(&mut parent, s), find(&mut parent, e));
            if rs != re {
                parent[rs as usize] = re;
            }
        }
        used[a as usize] = true;
        used[b as usize] = true;
        used[c as usize] = true;
    }

    for &(count, balance) in edges.values() {
        match count {
            1 => v.boundary_edges += 1,
            2 => {
                if balance != 0 {
                    v.misoriented_edges += 1;
                }
            }
            _ => v.non_manifold_edges += 1,
        }
    }

    let used_vertices = used.iter().filter(|&&u| u).count();
    let mut roots = std::collections::HashSet::new();
    for i in 0..welded_count as u32 {
        if used[i as usize] {
            roots.insert(find(&mut parent, i));
        }
    }
    v.components = roots.len();
    let non_degenerate = triangle_count - v.degenerate_triangles;
    v.euler_characteristic =
        used_vertices as i64 - edges.len() as i64 + non_degenerate as i64;
    v.watertight = v.boundary_edges == 0
        && v.non_manifold_edges == 0
        && v.misoriented_edges == 0
        && v.degenerate_triangles == 0
        && non_degenerate > 0;
    v
}

/// Map every vertex onto a canonical id by *exact* f32 position bits.
/// Returns `(weld[vertex] → canonical id, canonical count)`.
fn weld_by_position(mesh: &Mesh) -> (Vec<u32>, usize) {
    let vertex_count = mesh.vertex_count();
    let mut canon_of_pos: HashMap<[u32; 3], u32> = HashMap::new();
    let mut weld: Vec<u32> = Vec::with_capacity(vertex_count);
    for i in 0..vertex_count {
        let key = [
            mesh.positions[i * 3].to_bits(),
            mesh.positions[i * 3 + 1].to_bits(),
            mesh.positions[i * 3 + 2].to_bits(),
        ];
        let next = canon_of_pos.len() as u32;
        weld.push(*canon_of_pos.entry(key).or_insert(next));
    }
    let n = canon_of_pos.len();
    (weld, n)
}

/// Repair inconsistent triangle winding: propagate a consistent
/// orientation across shared (2-manifold) edges per connected component,
/// then flip whole components whose winding normals oppose the SDF
/// gradient (majority vote). Returns the number of triangles flipped.
///
/// Manifold-DC emits inconsistently-wound triangles along sharp feature
/// edges (measured: a plain box at export resolution has ~474
/// same-direction shared edges). The viewport renders `DoubleSide`, so
/// this was invisible — but STL/3MF consumers treat winding as the
/// outward orientation, so the export pass must repair it.
pub fn fix_orientation(mesh: &mut Mesh, sdf: &dyn Fn([f32; 3]) -> f32) -> usize {
    let tc = mesh.triangle_count();
    if tc == 0 {
        return 0;
    }
    let (weld, _) = weld_by_position(mesh);

    // Undirected welded edge → up to two incident (triangle, direction)
    // pairs. Edges with >2 incidents are non-manifold; they carry no
    // reliable orientation constraint and are skipped.
    let mut edge_tris: HashMap<(u32, u32), Vec<(u32, i32)>> = HashMap::new();
    for (t, tri) in mesh.indices.chunks_exact(3).enumerate() {
        let a = weld[tri[0] as usize];
        let b = weld[tri[1] as usize];
        let c = weld[tri[2] as usize];
        if a == b || b == c || c == a {
            continue;
        }
        for (s, e) in [(a, b), (b, c), (c, a)] {
            let (key, dir) = if s < e { ((s, e), 1) } else { ((e, s), -1) };
            edge_tris.entry(key).or_default().push((t as u32, dir));
        }
    }

    // BFS orientation propagation. `flip[t]`: triangle t must be flipped
    // relative to its as-authored winding to be consistent with its
    // component. Conflicts (odd cycles) are left as authored — they
    // surface in the post-repair validation.
    let mut flip = vec![false; tc];
    let mut visited = vec![false; tc];
    let mut components: Vec<Vec<u32>> = Vec::new();
    let mut queue = std::collections::VecDeque::new();
    for start in 0..tc {
        if visited[start] || mesh.indices[start * 3] == mesh.indices[start * 3 + 1] {
            continue;
        }
        visited[start] = true;
        queue.push_back(start as u32);
        let mut comp = vec![start as u32];
        while let Some(t) = queue.pop_front() {
            let tri = &mesh.indices[t as usize * 3..t as usize * 3 + 3];
            let a = weld[tri[0] as usize];
            let b = weld[tri[1] as usize];
            let c = weld[tri[2] as usize];
            for (s, e) in [(a, b), (b, c), (c, a)] {
                let (key, dir) = if s < e { ((s, e), 1) } else { ((e, s), -1) };
                let Some(incident) = edge_tris.get(&key) else {
                    continue;
                };
                if incident.len() != 2 {
                    continue;
                }
                for &(other, other_dir) in incident {
                    if other == t || visited[other as usize] {
                        continue;
                    }
                    // Consistent orientation traverses a shared edge in
                    // opposite directions: same effective direction ⇒
                    // the neighbour is flipped relative to us.
                    let my_eff = if flip[t as usize] { -dir } else { dir };
                    flip[other as usize] = my_eff == other_dir;
                    visited[other as usize] = true;
                    comp.push(other);
                    queue.push_back(other);
                }
            }
        }
        components.push(comp);
    }

    // Per component: area-weighted vote of (winding normal · SDF
    // gradient). Negative ⇒ the whole component is inside-out.
    let vert = |i: u32| -> [f32; 3] {
        let b = i as usize * 3;
        [
            mesh.positions[b],
            mesh.positions[b + 1],
            mesh.positions[b + 2],
        ]
    };
    for comp in &components {
        let stride = (comp.len() / 256).max(1);
        let mut vote = 0.0_f64;
        for &t in comp.iter().step_by(stride) {
            let a = vert(mesh.indices[t as usize * 3]);
            let b = vert(mesh.indices[t as usize * 3 + 1]);
            let c = vert(mesh.indices[t as usize * 3 + 2]);
            let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            let w = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
            let mut n = [
                u[1] * w[2] - u[2] * w[1],
                u[2] * w[0] - u[0] * w[2],
                u[0] * w[1] - u[1] * w[0],
            ];
            if flip[t as usize] {
                n = [-n[0], -n[1], -n[2]];
            }
            let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            if len < 1e-12 {
                continue;
            }
            let cen = [
                (a[0] + b[0] + c[0]) / 3.0,
                (a[1] + b[1] + c[1]) / 3.0,
                (a[2] + b[2] + c[2]) / 3.0,
            ];
            // Central-difference gradient scaled to the triangle size.
            let h = (len.sqrt() * 0.05).max(1e-4);
            let g = [
                sdf([cen[0] + h, cen[1], cen[2]]) - sdf([cen[0] - h, cen[1], cen[2]]),
                sdf([cen[0], cen[1] + h, cen[2]]) - sdf([cen[0], cen[1] - h, cen[2]]),
                sdf([cen[0], cen[1], cen[2] + h]) - sdf([cen[0], cen[1], cen[2] - h]),
            ];
            // n is area-weighted (len = 2·area), so big triangles count
            // proportionally more.
            vote += (n[0] * g[0] + n[1] * g[1] + n[2] * g[2]) as f64;
        }
        if vote < 0.0 {
            for &t in comp {
                flip[t as usize] = !flip[t as usize];
            }
        }
    }

    let mut flipped = 0;
    for (t, f) in flip.iter().enumerate() {
        if *f {
            mesh.indices.swap(t * 3 + 1, t * 3 + 2);
            flipped += 1;
        }
    }
    flipped
}

/// Measure the mesh's geometric deviation from the true surface by
/// evaluating the exact SDF at triangle centroids and edge midpoints —
/// the piecewise-linear surface is exact at the (Newton-projected)
/// vertices, so mid-face/mid-edge is where the chordal error lives.
/// Triangles are strided deterministically when the sample budget is
/// smaller than 4·triangles. Returns (max, p95, sample count).
pub fn measure_deviation(
    mesh: &Mesh,
    max_samples: usize,
    sdf: &dyn Fn([f32; 3]) -> f32,
) -> (f32, f32, usize) {
    let tc = mesh.triangle_count();
    if tc == 0 || max_samples == 0 {
        return (0.0, 0.0, 0);
    }
    let stride = (tc * 4).div_ceil(max_samples).max(1);
    let vert = |i: u32| -> [f32; 3] {
        let b = i as usize * 3;
        [
            mesh.positions[b],
            mesh.positions[b + 1],
            mesh.positions[b + 2],
        ]
    };
    let mut deviations: Vec<f32> = Vec::with_capacity(tc / stride * 4 + 4);
    for t in (0..tc).step_by(stride) {
        let a = vert(mesh.indices[t * 3]);
        let b = vert(mesh.indices[t * 3 + 1]);
        let c = vert(mesh.indices[t * 3 + 2]);
        let centroid = [
            (a[0] + b[0] + c[0]) / 3.0,
            (a[1] + b[1] + c[1]) / 3.0,
            (a[2] + b[2] + c[2]) / 3.0,
        ];
        let mid = |p: [f32; 3], q: [f32; 3]| {
            [
                (p[0] + q[0]) * 0.5,
                (p[1] + q[1]) * 0.5,
                (p[2] + q[2]) * 0.5,
            ]
        };
        for p in [centroid, mid(a, b), mid(b, c), mid(c, a)] {
            deviations.push(sdf(p).abs());
        }
    }
    if deviations.is_empty() {
        return (0.0, 0.0, 0);
    }
    deviations.sort_by(|x, y| x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal));
    let n = deviations.len();
    let max = deviations[n - 1];
    let p95 = deviations[((n as f32 * 0.95) as usize).min(n - 1)];
    (max, p95, n)
}

/// Count PROPER self-intersections: pairs of non-adjacent triangles
/// (no shared welded vertex) that geometrically intersect. Broad phase
/// is a uniform grid hash over triangle AABBs; narrow phase is the
/// Möller segment-interval test. Coplanar overlaps are not counted
/// (they surface as duplicate/degenerate topology instead). Returns
/// `(intersections, fully_checked)` — the pair budget keeps worst-case
/// meshes from stalling the export; a truncated check is reported
/// honestly.
pub fn count_self_intersections(mesh: &Mesh, pair_budget: usize) -> (usize, bool) {
    let tc = mesh.triangle_count();
    if tc < 2 {
        return (0, true);
    }
    let (weld, _) = weld_by_position(mesh);
    let vert = |i: u32| -> [f64; 3] {
        let b = i as usize * 3;
        [
            mesh.positions[b] as f64,
            mesh.positions[b + 1] as f64,
            mesh.positions[b + 2] as f64,
        ]
    };

    // Grid cell = ~2× the median edge length (sampled).
    let mut sample_len = 0.0f64;
    let mut samples = 0usize;
    let stride = (tc / 512).max(1);
    for t in (0..tc).step_by(stride) {
        let a = vert(mesh.indices[t * 3]);
        let b = vert(mesh.indices[t * 3 + 1]);
        let d = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        sample_len += (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        samples += 1;
    }
    let cell = (sample_len / samples.max(1) as f64).max(1e-6) * 2.0;

    let key_of = |p: [f64; 3]| -> [i64; 3] {
        [
            (p[0] / cell).floor() as i64,
            (p[1] / cell).floor() as i64,
            (p[2] / cell).floor() as i64,
        ]
    };
    let mut grid: HashMap<[i64; 3], Vec<u32>> = HashMap::new();
    for t in 0..tc {
        let (a, b, c) = (
            vert(mesh.indices[t * 3]),
            vert(mesh.indices[t * 3 + 1]),
            vert(mesh.indices[t * 3 + 2]),
        );
        let lo = key_of([
            a[0].min(b[0]).min(c[0]),
            a[1].min(b[1]).min(c[1]),
            a[2].min(b[2]).min(c[2]),
        ]);
        let hi = key_of([
            a[0].max(b[0]).max(c[0]),
            a[1].max(b[1]).max(c[1]),
            a[2].max(b[2]).max(c[2]),
        ]);
        for x in lo[0]..=hi[0] {
            for y in lo[1]..=hi[1] {
                for z in lo[2]..=hi[2] {
                    grid.entry([x, y, z]).or_default().push(t as u32);
                }
            }
        }
    }

    let shares_vertex = |t1: usize, t2: usize| -> bool {
        for i in 0..3 {
            for j in 0..3 {
                if weld[mesh.indices[t1 * 3 + i] as usize]
                    == weld[mesh.indices[t2 * 3 + j] as usize]
                {
                    return true;
                }
            }
        }
        false
    };

    let mut tested: std::collections::HashSet<(u32, u32)> = std::collections::HashSet::new();
    let mut hits = 0usize;
    let mut complete = true;
    'outer: for bucket in grid.values() {
        for i in 0..bucket.len() {
            for j in (i + 1)..bucket.len() {
                let (t1, t2) = (bucket[i].min(bucket[j]), bucket[i].max(bucket[j]));
                if !tested.insert((t1, t2)) {
                    continue;
                }
                if tested.len() > pair_budget {
                    complete = false;
                    break 'outer;
                }
                let (t1, t2) = (t1 as usize, t2 as usize);
                if shares_vertex(t1, t2) {
                    continue;
                }
                let tri1 = [
                    vert(mesh.indices[t1 * 3]),
                    vert(mesh.indices[t1 * 3 + 1]),
                    vert(mesh.indices[t1 * 3 + 2]),
                ];
                let tri2 = [
                    vert(mesh.indices[t2 * 3]),
                    vert(mesh.indices[t2 * 3 + 1]),
                    vert(mesh.indices[t2 * 3 + 2]),
                ];
                if tri_tri_intersect(&tri1, &tri2) {
                    hits += 1;
                }
            }
        }
    }
    (hits, complete)
}

/// Möller-style triangle-triangle intersection (proper crossings only;
/// coplanar pairs return false).
fn tri_tri_intersect(t1: &[[f64; 3]; 3], t2: &[[f64; 3]; 3]) -> bool {
    fn sub3(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
        [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
    }
    fn cross3(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    }
    fn dot3(a: [f64; 3], b: [f64; 3]) -> f64 {
        a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
    }
    const EPS: f64 = 1e-12;

    // Signed distances of t2's vertices to t1's plane.
    let n1 = cross3(sub3(t1[1], t1[0]), sub3(t1[2], t1[0]));
    let d2: Vec<f64> = t2.iter().map(|&p| dot3(n1, sub3(p, t1[0]))).collect();
    if (d2[0] > EPS && d2[1] > EPS && d2[2] > EPS)
        || (d2[0] < -EPS && d2[1] < -EPS && d2[2] < -EPS)
    {
        return false;
    }
    // And vice versa.
    let n2 = cross3(sub3(t2[1], t2[0]), sub3(t2[2], t2[0]));
    let d1: Vec<f64> = t1.iter().map(|&p| dot3(n2, sub3(p, t2[0]))).collect();
    if (d1[0] > EPS && d1[1] > EPS && d1[2] > EPS)
        || (d1[0] < -EPS && d1[1] < -EPS && d1[2] < -EPS)
    {
        return false;
    }
    // Coplanar: skip (handled by topology/degeneracy checks).
    if d1.iter().all(|d| d.abs() <= EPS) || d2.iter().all(|d| d.abs() <= EPS) {
        return false;
    }

    // Interval overlap on the intersection line L = n1 × n2.
    let dir = cross3(n1, n2);
    let axis = {
        let a = [dir[0].abs(), dir[1].abs(), dir[2].abs()];
        if a[0] >= a[1] && a[0] >= a[2] {
            0
        } else if a[1] >= a[2] {
            1
        } else {
            2
        }
    };
    // Projected interval of a triangle on L.
    let interval = |t: &[[f64; 3]; 3], d: &[f64]| -> Option<(f64, f64)> {
        // Find the lone vertex on one side; the two crossing edges give
        // the interval endpoints.
        let mut lo = f64::INFINITY;
        let mut hi = f64::NEG_INFINITY;
        let mut found = false;
        for i in 0..3 {
            for j in (i + 1)..3 {
                if (d[i] > EPS && d[j] < -EPS) || (d[i] < -EPS && d[j] > EPS) {
                    let f = d[i] / (d[i] - d[j]);
                    let p = t[i][axis] + f * (t[j][axis] - t[i][axis]);
                    lo = lo.min(p);
                    hi = hi.max(p);
                    found = true;
                } else if d[i].abs() <= EPS {
                    lo = lo.min(t[i][axis]);
                    hi = hi.max(t[i][axis]);
                    found = true;
                } else if d[j].abs() <= EPS {
                    lo = lo.min(t[j][axis]);
                    hi = hi.max(t[j][axis]);
                    found = true;
                }
            }
        }
        if found {
            Some((lo, hi))
        } else {
            None
        }
    };
    let (Some((a0, a1)), Some((b0, b1))) = (interval(t1, &d1), interval(t2, &d2)) else {
        return false;
    };
    // Proper overlap (touching at a point does not count).
    a0.max(b0) + EPS < a1.min(b1)
}

// ─── tests ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// A unit tetrahedron — the smallest closed, consistently-oriented
    /// triangle mesh (outward-facing winding).
    fn tetrahedron() -> Mesh {
        Mesh {
            positions: vec![
                0.0, 0.0, 0.0, // 0
                1.0, 0.0, 0.0, // 1
                0.0, 1.0, 0.0, // 2
                0.0, 0.0, 1.0, // 3
            ],
            normals: vec![0.0; 12],
            // Outward winding: base (viewed from below) + three sides.
            indices: vec![0, 2, 1, 0, 1, 3, 1, 2, 3, 0, 3, 2],
            classifications: vec![1; 4],
            contributors: vec![],
            face_index: vec![],
        }
    }

    #[test]
    fn tetrahedron_is_watertight() {
        let v = validate_topology(&tetrahedron());
        assert!(v.watertight, "{v:?}");
        assert_eq!(v.boundary_edges, 0);
        assert_eq!(v.non_manifold_edges, 0);
        assert_eq!(v.misoriented_edges, 0);
        assert_eq!(v.components, 1);
        assert_eq!(v.euler_characteristic, 2);
    }

    #[test]
    fn missing_face_reports_boundary_edges() {
        let mut mesh = tetrahedron();
        mesh.indices.truncate(9); // drop the last face
        let v = validate_topology(&mesh);
        assert!(!v.watertight);
        assert_eq!(v.boundary_edges, 3);
    }

    #[test]
    fn flipped_face_reports_misoriented_edges() {
        let mut mesh = tetrahedron();
        mesh.indices[9..12].reverse(); // flip the last face's winding
        let v = validate_topology(&mesh);
        assert!(!v.watertight);
        assert_eq!(v.misoriented_edges, 3);
        assert_eq!(v.boundary_edges, 0);
    }

    #[test]
    fn degenerate_triangle_detected() {
        let mut mesh = tetrahedron();
        mesh.indices.extend_from_slice(&[0, 0, 1]);
        let v = validate_topology(&mesh);
        assert_eq!(v.degenerate_triangles, 1);
        assert!(!v.watertight);
    }

    #[test]
    fn two_shells_count_two_components() {
        let mut mesh = tetrahedron();
        let second = tetrahedron();
        let offset = mesh.vertex_count() as u32;
        for i in 0..second.vertex_count() {
            mesh.positions.push(second.positions[i * 3] + 10.0);
            mesh.positions.push(second.positions[i * 3 + 1]);
            mesh.positions.push(second.positions[i * 3 + 2]);
        }
        mesh.indices
            .extend(second.indices.iter().map(|&i| i + offset));
        let v = validate_topology(&mesh);
        assert!(v.watertight);
        assert_eq!(v.components, 2);
        assert_eq!(v.euler_characteristic, 4);
    }

    #[test]
    fn coincident_duplicate_vertices_weld_before_the_check() {
        // The same tetrahedron, but every face carries its own copies
        // of the vertices (triangle soup). Position-welding must
        // reunite them into a watertight shell.
        let tet = tetrahedron();
        let mut soup = Mesh::default();
        for tri in tet.indices.chunks_exact(3) {
            for &i in tri {
                let b = i as usize * 3;
                let n = (soup.positions.len() / 3) as u32;
                soup.positions
                    .extend_from_slice(&tet.positions[b..b + 3]);
                soup.indices.push(n);
            }
        }
        soup.normals = vec![0.0; soup.positions.len()];
        soup.classifications = vec![1; soup.positions.len() / 3];
        let v = validate_topology(&soup);
        assert!(v.watertight, "{v:?}");
        assert_eq!(v.components, 1);
    }

    #[test]
    fn deviation_measures_sphere_chord_sag() {
        // Icosahedron-ish sampling of a radius-10 sphere: vertices on
        // the surface, so all deviation lives mid-face. Use a coarse
        // tetrahedron inscribed in the sphere — centroids sit well
        // inside, giving a large, predictable |SDF|.
        let r = 10.0_f32;
        let s = r / 3.0_f32.sqrt();
        let mesh = Mesh {
            positions: vec![
                s, s, s, //
                s, -s, -s, //
                -s, s, -s, //
                -s, -s, s,
            ],
            normals: vec![0.0; 12],
            indices: vec![0, 1, 2, 0, 3, 1, 0, 2, 3, 1, 3, 2],
            classifications: vec![1; 4],
            contributors: vec![],
            face_index: vec![],
        };
        let sphere = |p: [f32; 3]| -> f32 {
            (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt() - r
        };
        let (max, p95, n) = measure_deviation(&mesh, 1000, &sphere);
        assert_eq!(n, 16); // 4 triangles × 4 samples
        // Face centroid of an inscribed tetrahedron sits at r/3 from
        // the centre → deviation r − r/3 = 2r/3 ≈ 6.67.
        assert!((max - 2.0 * r / 3.0).abs() < 0.1, "max {max}");
        assert!(p95 <= max);
    }

    #[test]
    fn deviation_respects_sample_budget() {
        let mesh = tetrahedron();
        let flat = |_: [f32; 3]| 0.0_f32;
        let (_, _, n) = measure_deviation(&mesh, 4, &flat);
        assert!(n <= 8, "stride must cap samples, got {n}");
    }

    #[test]
    fn self_intersection_detects_a_piercing_triangle() {
        // Closed tetrahedron + a lone triangle stabbing through one of
        // its faces (no shared vertices).
        let mut mesh = tetrahedron();
        let n = mesh.vertex_count() as u32;
        mesh.positions.extend_from_slice(&[
            0.25, -0.5, 0.25, // below the base
            0.25, 0.9, 0.25, // up through the interior
            0.3, -0.5, 0.35,
        ]);
        mesh.indices.extend_from_slice(&[n, n + 1, n + 2]);
        mesh.classifications = vec![1; mesh.vertex_count()];
        let (hits, complete) = count_self_intersections(&mesh, 1_000_000);
        assert!(complete);
        // The spike crosses the base and one slanted face.
        assert!(hits >= 1, "hits {hits}");
    }

    #[test]
    fn self_intersection_clean_on_closed_tetrahedron() {
        let (hits, complete) = count_self_intersections(&tetrahedron(), 1_000_000);
        assert!(complete);
        assert_eq!(hits, 0);
    }

    #[test]
    fn empty_mesh_is_not_watertight() {
        let v = validate_topology(&Mesh::default());
        assert!(!v.watertight);
        assert_eq!(v.triangles, 0);
    }
}
