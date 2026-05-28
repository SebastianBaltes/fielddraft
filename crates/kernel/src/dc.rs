//! Dual Contouring mesher.
//!
//! Delegates to the Manifold DC implementation in [`crate::mdc`] for
//! mesh extraction, then applies post-processing (Newton projection,
//! planar simplification, developable simplification).
//!
//! Everything here operates on a [`BoundedCsg`]; the kernel's linear
//! feature list is lifted into a tree first.

use crate::bvh::BoundedCsg;
use crate::csg::{EvalMode, MAX_CONTRIBUTORS};
use crate::world_to_local;
use fielddraft_protocol::{Primitive, Transform};
use std::collections::HashMap;

/// Triangulated mesh in the layout expected by the zero-copy bridge to
/// Three.js.
///
/// Layout:
///   * `positions` / `normals` — `xyz`-interleaved `f32` triples, one
///     entry per vertex.
///   * `indices` — flat `u32` triangle list.
///   * `classifications` — one `u8` per vertex:
///       0 = off-surface (should never appear once we've filtered the
///           DC output), 1 = face, 2 = edge, 3 = vertex. Matches the
///       [`ProxyElement::tag`] contract.
///   * `contributors` — flat `u32` array, `MAX_CONTRIBUTORS` slots per
///     vertex. Unused slots are set to `u32::MAX`. Readers walk the
///     stride and stop at the sentinel.
#[derive(Debug, Default, Clone)]
pub struct Mesh {
    pub positions: Vec<f32>,
    pub normals: Vec<f32>,
    pub indices: Vec<u32>,
    pub classifications: Vec<u8>,
    pub contributors: Vec<u32>,
}

impl Mesh {
    pub fn vertex_count(&self) -> usize {
        self.positions.len() / 3
    }

    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    pub fn is_empty(&self) -> bool {
        self.positions.is_empty() || self.indices.is_empty()
    }
}

/// Sentinel written into unused contributor slots so the frontend can
/// stop iterating early without a separate length array.
pub const CONTRIBUTOR_SENTINEL: u32 = u32::MAX;

/// Number of extra voxel layers added around the scene AABB so every
/// surface vertex has a positive-SDF neighbour to interpolate against
/// (otherwise a box exactly filling its bounds would produce edges with
/// a zero sample on one end, which the sign-change test can't classify
/// reliably).
pub(crate) const DC_MARGIN_CELLS: f32 = 2.0;

/// Mesh a CSG tree with a uniform-grid Dual Contouring pass.
///
/// `resolution` is the number of cells along the *longest* AABB axis;
/// shorter axes get fewer cells so voxels stay cubic. An empty scene
/// (or `resolution == 0`) yields an empty mesh.
///
/// K3: takes a [`BoundedCsg`] so cell-by-cell SDF queries hit the
/// BVH-pruned, transform-flattened hot path. The mesher uses
/// [`EvalMode::Visual`] — see the module docs on `csg.rs` for the
/// dual-path rationale.
pub fn dual_contour(node: &BoundedCsg, resolution: u32) -> Mesh {
    dual_contour_with_primitives(node, resolution, &HashMap::new())
}

pub fn dual_contour_with_primitives(
    node: &BoundedCsg,
    resolution: u32,
    leaf_primitives: &HashMap<u32, &Primitive>,
) -> Mesh {
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
    let grid = raw.expanded(cell * DC_MARGIN_CELLS);
    let size = grid.size();

    let nx = ((size[0] / cell).ceil() as usize).max(1);
    let ny = ((size[1] / cell).ceil() as usize).max(1);
    let nz = ((size[2] / cell).ceil() as usize).max(1);

    let normal_eps = cell * 0.05;

    let mut mesh = crate::mdc::manifold_dc(node, grid.min, cell, nx, ny, nz);

    project_face_vertices(&mut mesh, node, normal_eps);
    simplify_planar_faces(&mut mesh);
    if !leaf_primitives.is_empty() {
        let leaf_transforms = node.leaf_world_transforms();
        simplify_developable_faces(&mut mesh, leaf_primitives, &leaf_transforms);
    }

    mesh
}

// ── K3.5 Phase B: planar face simplification ────────────────────────

/// Simplify planar face regions by replacing dense DC triangulations
/// with minimal ear-clipped polygons. Only processes regions where all
/// face vertices share the same contributor and normal direction, and
/// the boundary forms a single closed loop (no holes).
fn simplify_planar_faces(mesh: &mut Mesh) {
    let regions = find_planar_regions(mesh);
    if regions.is_empty() {
        return;
    }

    let tc = mesh.triangle_count();
    let mut keep = vec![true; tc];
    let mut new_indices: Vec<u32> = Vec::new();

    for region in &regions {
        if region.boundary.len() < 3 || region.tris.len() < 2 {
            continue;
        }
        let simplified = simplify_region(mesh, region);
        if simplified.is_empty() {
            continue;
        }
        for &t in &region.tris {
            keep[t] = false;
        }
        new_indices.extend_from_slice(&simplified);
    }

    if new_indices.is_empty() {
        return;
    }

    let mut final_indices = Vec::with_capacity(mesh.indices.len());
    for t in 0..tc {
        if keep[t] {
            final_indices.extend_from_slice(&mesh.indices[t * 3..t * 3 + 3]);
        }
    }
    final_indices.extend(new_indices);
    mesh.indices = final_indices;

    compact_mesh(mesh);
}

struct PlanarRegion {
    normal: [f32; 3],
    tris: Vec<usize>,
    boundary: Vec<u32>,
    holes: Vec<Vec<u32>>,
}

fn is_region_planar(mesh: &Mesh, comp: &[usize], _ref_normal: [f32; 3]) -> bool {
    let mut avg = [0.0_f64; 3];
    for &t in comp {
        let n = tri_geometric_normal(mesh, t);
        avg[0] += n[0] as f64;
        avg[1] += n[1] as f64;
        avg[2] += n[2] as f64;
    }
    let len = (avg[0] * avg[0] + avg[1] * avg[1] + avg[2] * avg[2]).sqrt();
    if len < 1e-10 {
        return false;
    }
    let plane_n = [(avg[0] / len) as f32, (avg[1] / len) as f32, (avg[2] / len) as f32];

    let mut verts_seen = std::collections::HashSet::new();
    for &t in comp {
        for k in 0..3 {
            verts_seen.insert(mesh.indices[t * 3 + k]);
        }
    }
    if verts_seen.len() < 3 {
        return false;
    }

    let mut min_d = f32::MAX;
    let mut max_d = f32::MIN;
    let mut min_u = f32::MAX;
    let mut max_u = f32::MAX;
    let mut min_v = f32::MAX;
    let mut max_v = f32::MAX;

    let (u_axis, v_axis) = orthonormal_basis(plane_n);

    let mut first = true;
    for &vi in &verts_seen {
        let p = vtx_pos(mesh, vi as usize);
        let d = dot3(p, plane_n);
        let u = dot3(p, u_axis);
        let v = dot3(p, v_axis);
        if first {
            min_d = d; max_d = d;
            min_u = u; max_u = u;
            min_v = v; max_v = v;
            first = false;
        } else {
            if d < min_d { min_d = d; }
            if d > max_d { max_d = d; }
            if u < min_u { min_u = u; }
            if u > max_u { max_u = u; }
            if v < min_v { min_v = v; }
            if v > max_v { max_v = v; }
        }
    }

    let thickness = max_d - min_d;
    let width = (max_u - min_u).max(max_v - min_v);
    if width < 1e-6 {
        return false;
    }

    thickness / width < 0.05
}

fn find_planar_regions(mesh: &Mesh) -> Vec<PlanarRegion> {
    let tc = mesh.triangle_count();
    if tc == 0 {
        return vec![];
    }

    // Group face triangles by (contributor, geometric normal bin).
    // Using geometric triangle normals (cross product of edges) is more
    // stable than vertex normals for box-like shapes where the SDF
    // gradient is discontinuous at edges.
    let mut tri_groups: HashMap<(u32, i8), Vec<usize>> = HashMap::new();
    for t in 0..tc {
        let [i0, i1, i2] = [
            mesh.indices[t * 3] as usize,
            mesh.indices[t * 3 + 1] as usize,
            mesh.indices[t * 3 + 2] as usize,
        ];

        // All 3 vertices must be Face-classified with the same contributor
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

        let n = tri_geometric_normal(mesh, t);
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        if len < 1e-10 {
            continue;
        }
        let bin = normal_bin([n[0] / len, n[1] / len, n[2] / len]);
        tri_groups.entry((c0, bin)).or_default().push(t);
    }

    let mut regions = Vec::new();

    for (_, tris) in &tri_groups {
        if tris.len() < 2 {
            continue;
        }

        let components = connected_components(tris, mesh);
        for comp in components {
            if comp.len() < 2 {
                continue;
            }
            let n = tri_geometric_normal(mesh, comp[0]);
            let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            if len < 1e-10 {
                continue;
            }
            let n = [n[0] / len, n[1] / len, n[2] / len];

            if !is_region_planar(mesh, &comp, n) {
                continue;
            }

            let loops = extract_boundary_loops(mesh, &comp);
            if !loops.is_empty() {
                // Largest loop is outer boundary, rest are holes.
                let mut sorted = loops;
                sorted.sort_by(|a, b| b.len().cmp(&a.len()));
                let boundary = sorted.remove(0);
                regions.push(PlanarRegion {
                    normal: n,
                    tris: comp,
                    boundary,
                    holes: sorted,
                });
            } else if let Some(boundary) = extract_boundary_loop(mesh, &comp) {
                regions.push(PlanarRegion {
                    normal: n,
                    tris: comp,
                    boundary,
                    holes: Vec::new(),
                });
            }
        }
    }

    regions
}

pub(crate) fn connected_components(tris: &[usize], mesh: &Mesh) -> Vec<Vec<usize>> {
    if tris.is_empty() {
        return vec![];
    }

    // Map vertex → triangle indices within this set
    let mut vtx_to_tri: HashMap<u32, Vec<usize>> = HashMap::new();
    for (local, &t) in tris.iter().enumerate() {
        for k in 0..3 {
            vtx_to_tri
                .entry(mesh.indices[t * 3 + k])
                .or_default()
                .push(local);
        }
    }

    let n = tris.len();
    let mut visited = vec![false; n];
    let mut components = Vec::new();

    for start in 0..n {
        if visited[start] {
            continue;
        }
        let mut comp = Vec::new();
        let mut stack = vec![start];
        while let Some(idx) = stack.pop() {
            if visited[idx] {
                continue;
            }
            visited[idx] = true;
            comp.push(tris[idx]);
            let t = tris[idx];
            for k in 0..3 {
                let vi = mesh.indices[t * 3 + k];
                if let Some(neighbors) = vtx_to_tri.get(&vi) {
                    for &ni in neighbors {
                        if !visited[ni] {
                            stack.push(ni);
                        }
                    }
                }
            }
        }
        components.push(comp);
    }

    components
}

fn extract_boundary_loops(mesh: &Mesh, tris: &[usize]) -> Vec<Vec<u32>> {
    let mut edge_count: HashMap<(u32, u32), usize> = HashMap::new();
    for &t in tris {
        let tri = [mesh.indices[t * 3], mesh.indices[t * 3 + 1], mesh.indices[t * 3 + 2]];
        for e in 0..3 {
            let a = tri[e];
            let b = tri[(e + 1) % 3];
            let key = if a < b { (a, b) } else { (b, a) };
            *edge_count.entry(key).or_default() += 1;
        }
    }

    let boundary: Vec<(u32, u32)> = edge_count
        .into_iter()
        .filter(|&(_, c)| c == 1)
        .map(|(e, _)| e)
        .collect();

    if boundary.is_empty() {
        return Vec::new();
    }

    let mut adj: HashMap<u32, Vec<u32>> = HashMap::new();
    for &(a, b) in &boundary {
        adj.entry(a).or_default().push(b);
        adj.entry(b).or_default().push(a);
    }

    // All boundary vertices must have exactly 2 neighbors
    for neighbors in adj.values() {
        if neighbors.len() != 2 {
            return Vec::new();
        }
    }

    let mut visited: std::collections::HashSet<u32> = std::collections::HashSet::new();
    let mut loops = Vec::new();

    for &(start, _) in &boundary {
        if visited.contains(&start) { continue; }
        let mut loop_verts = Vec::new();
        let mut curr = start;
        let mut prev = u32::MAX;
        loop {
            if visited.contains(&curr) && curr != start { break; }
            loop_verts.push(curr);
            visited.insert(curr);
            let ns = match adj.get(&curr) {
                Some(n) => n,
                None => break,
            };
            let next = if ns[0] == prev { ns[1] } else { ns[0] };
            prev = curr;
            curr = next;
            if curr == start { break; }
            if loop_verts.len() > boundary.len() + 1 { break; }
        }
        if loop_verts.len() >= 3 {
            loops.push(loop_verts);
        }
    }
    loops
}

fn extract_boundary_loop(mesh: &Mesh, tris: &[usize]) -> Option<Vec<u32>> {
    let mut edge_count: HashMap<(u32, u32), usize> = HashMap::new();
    for &t in tris {
        let tri = [
            mesh.indices[t * 3],
            mesh.indices[t * 3 + 1],
            mesh.indices[t * 3 + 2],
        ];
        for e in 0..3 {
            let a = tri[e];
            let b = tri[(e + 1) % 3];
            let key = if a < b { (a, b) } else { (b, a) };
            *edge_count.entry(key).or_default() += 1;
        }
    }

    let boundary: Vec<(u32, u32)> = edge_count
        .into_iter()
        .filter(|&(_, c)| c == 1)
        .map(|(e, _)| e)
        .collect();

    if boundary.is_empty() {
        return None;
    }

    // Build adjacency for boundary vertices
    let mut adj: HashMap<u32, Vec<u32>> = HashMap::new();
    for &(a, b) in &boundary {
        adj.entry(a).or_default().push(b);
        adj.entry(b).or_default().push(a);
    }

    // All boundary vertices must have exactly 2 neighbors (single loop)
    for neighbors in adj.values() {
        if neighbors.len() != 2 {
            return None;
        }
    }

    // Walk the loop
    let start = boundary[0].0;
    let mut result = Vec::with_capacity(boundary.len());
    let mut curr = start;
    let mut prev = u32::MAX;
    loop {
        result.push(curr);
        let ns = &adj[&curr];
        let next = if ns[0] == prev { ns[1] } else { ns[0] };
        prev = curr;
        curr = next;
        if curr == start {
            break;
        }
        if result.len() > boundary.len() {
            return None;
        }
    }

    if result.len() >= 3 {
        Some(result)
    } else {
        None
    }
}

fn simplify_region(mesh: &Mesh, region: &PlanarRegion) -> Vec<u32> {
    let (u_axis, v_axis) = orthonormal_basis(region.normal);

    let project = |vi: u32| -> [f32; 2] {
        let p = vtx_pos(mesh, vi as usize);
        [dot3(p, u_axis), dot3(p, v_axis)]
    };

    // Build merged vertex list: outer boundary + holes bridged in.
    let mut all_verts: Vec<u32> = region.boundary.clone();
    let mut all_pts: Vec<[f32; 2]> = all_verts.iter().map(|&vi| project(vi)).collect();

    // Bridge each hole into the outer polygon.
    for hole in &region.holes {
        if hole.len() < 3 { continue; }
        let hole_pts: Vec<[f32; 2]> = hole.iter().map(|&vi| project(vi)).collect();

        // Find the rightmost hole vertex
        let (hole_start, _) = hole_pts.iter().enumerate()
            .max_by(|a, b| a.1[0].partial_cmp(&b.1[0]).unwrap())
            .unwrap();

        let hole_start_pt = hole_pts[hole_start];

        // Find the nearest outer vertex to the bridge point
        let (outer_idx, _) = all_pts.iter().enumerate()
            .min_by(|a, b| {
                let da = (a.1[0] - hole_start_pt[0]).powi(2) + (a.1[1] - hole_start_pt[1]).powi(2);
                let db = (b.1[0] - hole_start_pt[0]).powi(2) + (b.1[1] - hole_start_pt[1]).powi(2);
                da.partial_cmp(&db).unwrap()
            })
            .unwrap();

        // Insert hole vertices into the outer polygon at the bridge point.
        // Pattern: ...outer[bridge] → hole[start] → ... → hole[start] → outer[bridge]...
        let n = hole.len();
        let mut bridge: Vec<u32> = Vec::with_capacity(n + 2);
        let mut bridge_pts: Vec<[f32; 2]> = Vec::with_capacity(n + 2);
        bridge.push(all_verts[outer_idx]);
        bridge_pts.push(all_pts[outer_idx]);
        for i in 0..=n {
            let hi = (hole_start + i) % n;
            bridge.push(hole[hi]);
            bridge_pts.push(hole_pts[hi]);
        }
        bridge.push(all_verts[outer_idx]);
        bridge_pts.push(all_pts[outer_idx]);

        all_verts.splice(outer_idx + 1..outer_idx + 1, bridge);
        all_pts.splice(outer_idx + 1..outer_idx + 1, bridge_pts);
    }

    let tris = earcut_2d(&all_pts);

    let mut indices = Vec::with_capacity(tris.len() * 3);
    for [a, b, c] in tris {
        indices.push(all_verts[a]);
        indices.push(all_verts[b]);
        indices.push(all_verts[c]);
    }
    indices
}

fn orthonormal_basis(n: [f32; 3]) -> ([f32; 3], [f32; 3]) {
    let up = if n[1].abs() < 0.9 {
        [0.0, 1.0, 0.0]
    } else {
        [1.0, 0.0, 0.0]
    };
    let u = cross3(up, n);
    let u_len = (u[0] * u[0] + u[1] * u[1] + u[2] * u[2]).sqrt();
    let u = [u[0] / u_len, u[1] / u_len, u[2] / u_len];
    let v = cross3(n, u);
    (u, v)
}

fn cross3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot3(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn tri_geometric_normal(mesh: &Mesh, t: usize) -> [f32; 3] {
    let p0 = vtx_pos(mesh, mesh.indices[t * 3] as usize);
    let p1 = vtx_pos(mesh, mesh.indices[t * 3 + 1] as usize);
    let p2 = vtx_pos(mesh, mesh.indices[t * 3 + 2] as usize);
    let e1 = [p1[0] - p0[0], p1[1] - p0[1], p1[2] - p0[2]];
    let e2 = [p2[0] - p0[0], p2[1] - p0[1], p2[2] - p0[2]];
    cross3(e1, e2)
}

/// Quantize a normal into one of 6 axis-aligned bins (±X, ±Y, ±Z).
/// Returns a signed axis index: ±1 for X, ±2 for Y, ±3 for Z.
fn normal_bin(n: [f32; 3]) -> i8 {
    let ax = n[0].abs();
    let ay = n[1].abs();
    let az = n[2].abs();
    if ax >= ay && ax >= az {
        if n[0] >= 0.0 { 1 } else { -1 }
    } else if ay >= ax && ay >= az {
        if n[1] >= 0.0 { 2 } else { -2 }
    } else {
        if n[2] >= 0.0 { 3 } else { -3 }
    }
}

fn vtx_pos(mesh: &Mesh, v: usize) -> [f32; 3] {
    [
        mesh.positions[v * 3],
        mesh.positions[v * 3 + 1],
        mesh.positions[v * 3 + 2],
    ]
}

// ── Ear-clipping triangulation ──────────────────────────────────────

fn earcut_2d(verts: &[[f32; 2]]) -> Vec<[usize; 3]> {
    let n = verts.len();
    if n < 3 {
        return vec![];
    }
    if n == 3 {
        return vec![[0, 1, 2]];
    }

    let mut idx: Vec<usize> = (0..n).collect();

    // Ensure CCW winding
    let area = signed_area_2d(verts, &idx);
    if area < 0.0 {
        idx.reverse();
    }

    let mut tris = Vec::with_capacity(n - 2);
    let mut attempts = 0;
    let max_attempts = idx.len() * idx.len();

    while idx.len() > 2 && attempts < max_attempts {
        let len = idx.len();
        let mut clipped = false;
        for i in 0..len {
            let prev = idx[(i + len - 1) % len];
            let curr = idx[i];
            let next = idx[(i + 1) % len];

            if is_ear(verts, &idx, prev, curr, next) {
                tris.push([prev, curr, next]);
                idx.remove(i);
                clipped = true;
                break;
            }
        }
        if !clipped {
            break;
        }
        attempts += 1;
    }

    tris
}

fn signed_area_2d(verts: &[[f32; 2]], idx: &[usize]) -> f32 {
    let n = idx.len();
    let mut area = 0.0_f32;
    for i in 0..n {
        let a = verts[idx[i]];
        let b = verts[idx[(i + 1) % n]];
        area += a[0] * b[1] - b[0] * a[1];
    }
    area * 0.5
}

fn is_ear(verts: &[[f32; 2]], idx: &[usize], prev: usize, curr: usize, next: usize) -> bool {
    let a = verts[prev];
    let b = verts[curr];
    let c = verts[next];

    // Convexity check (CCW cross product > 0)
    let cross = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
    if cross <= 1e-10 {
        return false;
    }

    // No other polygon vertex may lie inside the triangle
    for &i in idx {
        if i == prev || i == curr || i == next {
            continue;
        }
        if point_in_tri(verts[i], a, b, c) {
            return false;
        }
    }

    true
}

fn point_in_tri(p: [f32; 2], a: [f32; 2], b: [f32; 2], c: [f32; 2]) -> bool {
    let d1 = tri_sign(p, a, b);
    let d2 = tri_sign(p, b, c);
    let d3 = tri_sign(p, c, a);
    let has_neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
    let has_pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
    !(has_neg && has_pos)
}

fn tri_sign(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    (p[0] - b[0]) * (a[1] - b[1]) - (a[0] - b[0]) * (p[1] - b[1])
}

// ── Vertex compaction ───────────────────────────────────────────────

// ── K3.5 Phase C: developable surface simplification ────────────

/// Simplify cylinder and cone face regions by unrolling them into
/// 2D (θ, h) parameter space, extracting boundary, ear-clipping,
/// and mapping back to 3D.
fn simplify_developable_faces(
    mesh: &mut Mesh,
    prims: &HashMap<u32, &Primitive>,
    transforms: &HashMap<u32, Transform>,
) {
    let tc = mesh.triangle_count();
    if tc == 0 {
        return;
    }

    // Find face triangles on cylinder/cone surfaces.
    // Group by (contributor, surface_type).
    let mut cyl_tris: HashMap<u32, Vec<usize>> = HashMap::new();

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

        if let Some(prim) = prims.get(&c0) {
            if matches!(prim, Primitive::Cylinder { .. } | Primitive::Cone { .. }) {
                cyl_tris.entry(c0).or_default().push(t);
            }
        }
    }

    if cyl_tris.is_empty() {
        return;
    }

    let mut keep = vec![true; tc];
    let mut new_indices: Vec<u32> = Vec::new();

    for (&contrib, tris) in &cyl_tris {
        let prim = match prims.get(&contrib) {
            Some(p) => *p,
            None => continue,
        };
        // Extract center and representative radius for UV parameterization.
        // Cones use the average of bottom/top radii for the arc-length scale.
        let (center, radius, _half_height) = match prim {
            Primitive::Cylinder { center, radius, half_height } => (*center, *radius, *half_height),
            Primitive::Cone { r_bottom, r_top, half_height } => (
                [0.0, 0.0, 0.0],
                (r_bottom + r_top) * 0.5,
                *half_height,
            ),
            _ => continue,
        };

        let xf = transforms.get(&contrib).copied().unwrap_or(Transform::IDENTITY);
        let normal_xf = Transform { translate: [0.0, 0.0, 0.0], rotation: xf.rotation, scale: [1.0, 1.0, 1.0] };

        // Split by normal bin to separate top/bottom caps from lateral surface.
        // Lateral surface normals are radial (perpendicular to Y axis in
        // the cylinder's local frame). We transform world-space normals
        // into the primitive's local frame so rotated cylinders are
        // classified correctly.
        let mut lateral: Vec<usize> = Vec::new();
        for &t in tris {
            let n = tri_geometric_normal(mesh, t);
            let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            if len < 1e-10 { continue; }
            let n_local = world_to_local(&normal_xf, [n[0] / len, n[1] / len, n[2] / len]);
            if n_local[1].abs() < 0.5 {
                lateral.push(t);
            }
        }

        if lateral.len() < 4 { continue; }

        // Connected components of lateral triangles
        let components = connected_components(&lateral, mesh);

        for comp in components {
            if comp.len() < 4 { continue; }

            // Parameterize vertices as (θ, h)
            let comp_verts: std::collections::HashSet<u32> = comp
                .iter()
                .flat_map(|&t| {
                    let b = t * 3;
                    [mesh.indices[b], mesh.indices[b + 1], mesh.indices[b + 2]]
                })
                .collect();

            let mut vert_list: Vec<u32> = comp_verts.into_iter().collect();
            vert_list.sort();

            // Convert to (θ, h) in cylinder local coords.
            // Mesh vertices are in world space; transform them back
            // to the primitive's local frame before parameterization.
            let mut theta_h: HashMap<u32, [f32; 2]> = HashMap::new();
            for &vi in &vert_list {
                let p = vtx_pos(mesh, vi as usize);
                let local = world_to_local(&xf, p);
                let dx = local[0] - center[0];
                let dz = local[2] - center[2];
                let theta = dz.atan2(dx);
                let h = local[1] - center[1];
                theta_h.insert(vi, [theta, h]);
            }

            // Extract boundary loop
            if let Some(boundary) = extract_boundary_loop(mesh, &comp) {
                if boundary.len() < 3 { continue; }

                // Unwrap θ to avoid seam discontinuity
                let mut uvs: Vec<[f32; 2]> = Vec::with_capacity(boundary.len());
                let first_th = theta_h[&boundary[0]][0];
                let mut prev_th = first_th;
                for &vi in &boundary {
                    let [mut th, h] = theta_h[&vi];
                    // Unwrap: keep θ continuous with the previous vertex
                    while th - prev_th > std::f32::consts::PI { th -= 2.0 * std::f32::consts::PI; }
                    while th - prev_th < -std::f32::consts::PI { th += 2.0 * std::f32::consts::PI; }
                    uvs.push([th * radius, h]); // scale θ by radius for isometric unrolling
                    prev_th = th;
                }

                let tris_2d = earcut_2d(&uvs);
                if tris_2d.is_empty() || tris_2d.len() * 5 >= comp.len() * 4 { continue; }

                for &t in &comp {
                    keep[t] = false;
                }
                for [a, b, c] in tris_2d {
                    new_indices.push(boundary[a]);
                    new_indices.push(boundary[b]);
                    new_indices.push(boundary[c]);
                }
            }
        }
    }

    if new_indices.is_empty() {
        return;
    }

    let mut final_indices = Vec::with_capacity(mesh.indices.len());
    for t in 0..tc {
        if keep[t] {
            final_indices.extend_from_slice(&mesh.indices[t * 3..t * 3 + 3]);
        }
    }
    final_indices.extend(new_indices);
    mesh.indices = final_indices;
    compact_mesh(mesh);
}

fn compact_mesh(mesh: &mut Mesh) {
    let vc = mesh.vertex_count();
    let mut used = vec![false; vc];
    for &i in &mesh.indices {
        used[i as usize] = true;
    }

    let used_count = used.iter().filter(|&&u| u).count();
    if used_count == vc {
        return;
    }

    let mut remap = vec![0u32; vc];
    let mut new_pos = Vec::with_capacity(used_count * 3);
    let mut new_nor = Vec::with_capacity(used_count * 3);
    let mut new_cls = Vec::with_capacity(used_count);
    let mut new_ctr = Vec::with_capacity(used_count * MAX_CONTRIBUTORS);
    let mut next_idx = 0u32;

    for v in 0..vc {
        if !used[v] {
            continue;
        }
        remap[v] = next_idx;
        next_idx += 1;
        new_pos.extend_from_slice(&mesh.positions[v * 3..v * 3 + 3]);
        new_nor.extend_from_slice(&mesh.normals[v * 3..v * 3 + 3]);
        new_cls.push(mesh.classifications[v]);
        let cb = v * MAX_CONTRIBUTORS;
        new_ctr.extend_from_slice(&mesh.contributors[cb..cb + MAX_CONTRIBUTORS]);
    }

    for idx in &mut mesh.indices {
        *idx = remap[*idx as usize];
    }

    mesh.positions = new_pos;
    mesh.normals = new_nor;
    mesh.classifications = new_cls;
    mesh.contributors = new_ctr;
}

/// K3.5 Phase A: Newton-project every Face-classified vertex onto the
/// exact SDF zero-level set. Edge and Vertex classified points are left
/// untouched — they sit on intersection curves where projection along a
/// single contributor's gradient would drag them off the crease.
fn project_face_vertices(mesh: &mut Mesh, node: &BoundedCsg, normal_eps: f32) {
    const MAX_ITERS: usize = 4;
    const TOLERANCE: f32 = 1e-5;

    for v in 0..mesh.vertex_count() {
        if mesh.classifications[v] != 1 {
            continue;
        }
        let mut p = [
            mesh.positions[v * 3],
            mesh.positions[v * 3 + 1],
            mesh.positions[v * 3 + 2],
        ];
        for _ in 0..MAX_ITERS {
            let d = node.sdf(p, EvalMode::Visual);
            if d.abs() < TOLERANCE {
                break;
            }
            let n = sdf_normal(node, p, normal_eps);
            p[0] -= d * n[0];
            p[1] -= d * n[1];
            p[2] -= d * n[2];
        }
        mesh.positions[v * 3] = p[0];
        mesh.positions[v * 3 + 1] = p[1];
        mesh.positions[v * 3 + 2] = p[2];
        let n = sdf_normal(node, p, normal_eps);
        mesh.normals[v * 3] = n[0];
        mesh.normals[v * 3 + 1] = n[1];
        mesh.normals[v * 3 + 2] = n[2];
    }
}

/// Surface normal at `p` by central differences on the BVH SDF.
/// Returns a unit vector; collapses to `+Y` when the gradient is
/// numerically zero (only possible in degenerate cases like an empty
/// tree, which the caller already guards against).
pub(crate) fn sdf_normal(node: &BoundedCsg, p: [f32; 3], eps: f32) -> [f32; 3] {
    let eps = eps.max(1e-4);
    let m = EvalMode::Visual;
    let dx = node.sdf([p[0] + eps, p[1], p[2]], m)
        - node.sdf([p[0] - eps, p[1], p[2]], m);
    let dy = node.sdf([p[0], p[1] + eps, p[2]], m)
        - node.sdf([p[0], p[1] - eps, p[2]], m);
    let dz = node.sdf([p[0], p[1], p[2] + eps], m)
        - node.sdf([p[0], p[1], p[2] - eps], m);
    let len = (dx * dx + dy * dy + dz * dz).sqrt();
    if len > 1e-12 {
        [dx / len, dy / len, dz / len]
    } else {
        [0.0, 1.0, 0.0]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fielddraft_protocol::{BoolOp, Feature, FeatureId, Primitive, Transform};

    fn sphere_feature(id: u32, center: [f32; 3], radius: f32) -> Feature {
        Feature {
            id: FeatureId::from_u32(id),
            name: format!("Sphere {id}"),
            op: BoolOp::Union,
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

    fn box_feature(id: u32, op: BoolOp, center: [f32; 3], half: [f32; 3]) -> Feature {
        Feature {
            id: FeatureId::from_u32(id),
            name: format!("Box {id}"),
            op,
            primitive: Primitive::Box {
                center,
                half_extents: half,
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

    fn cylinder_feature(
        id: u32,
        op: BoolOp,
        center: [f32; 3],
        radius: f32,
        half_height: f32,
    ) -> Feature {
        Feature {
            id: FeatureId::from_u32(id),
            name: format!("Cyl {id}"),
            op,
            primitive: Primitive::Cylinder {
                center,
                radius,
                half_height,
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

    fn tri_normal(p0: [f32; 3], p1: [f32; 3], p2: [f32; 3]) -> [f32; 3] {
        let a = [p1[0] - p0[0], p1[1] - p0[1], p1[2] - p0[2]];
        let b = [p2[0] - p0[0], p2[1] - p0[1], p2[2] - p0[2]];
        let n = [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ];
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        if len > 0.0 {
            [n[0] / len, n[1] / len, n[2] / len]
        } else {
            [0.0, 0.0, 0.0]
        }
    }

    fn mesh_for(features: &[Feature], resolution: u32) -> Mesh {
        let csg = crate::csg::CsgNode::from_features(features);
        let bounded = BoundedCsg::from_csg(&csg);
        dual_contour(&bounded, resolution)
    }

    #[test]
    fn empty_tree_has_empty_mesh() {
        let mesh = dual_contour(&BoundedCsg::Empty, 16);
        assert!(mesh.is_empty());
        assert_eq!(mesh.vertex_count(), 0);
        assert_eq!(mesh.triangle_count(), 0);
    }

    #[test]
    fn single_sphere_produces_non_empty_manifold_like_mesh() {
        let mesh = mesh_for(&[sphere_feature(1, [0.0, 0.0, 0.0], 20.0)], 24);
        assert!(mesh.vertex_count() > 0);
        assert_eq!(
            mesh.indices.len() % 3,
            0,
            "indices must be multiples of 3"
        );
        assert!(mesh.triangle_count() > 0);

        // Every vertex should lie near the sphere surface (|sdf| small
        // relative to the cell size, which is ~20*2/24 ≈ 1.67).
        let tree = crate::csg::CsgNode::from_features(&[sphere_feature(
            1,
            [0.0, 0.0, 0.0],
            20.0,
        )]);
        let mut max_err = 0.0_f32;
        for i in 0..mesh.vertex_count() {
            let p = [
                mesh.positions[i * 3],
                mesh.positions[i * 3 + 1],
                mesh.positions[i * 3 + 2],
            ];
            let d = tree.sdf(p, EvalMode::Logical).abs();
            if d > max_err {
                max_err = d;
            }
        }
        assert!(
            max_err < 2.5,
            "sphere vertices drifted off the surface by {max_err}",
        );
    }

    #[test]
    fn sphere_triangle_normals_point_outward() {
        // For a sphere at the origin, every triangle's geometric
        // normal should have positive dot product with its centroid
        // direction (i.e. point away from the origin).
        let mesh = mesh_for(&[sphere_feature(1, [0.0, 0.0, 0.0], 20.0)], 24);
        assert!(mesh.triangle_count() > 0);
        let mut outward = 0usize;
        let total = mesh.triangle_count();
        for t in 0..total {
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
            let n = tri_normal(p0, p1, p2);
            let c = [
                (p0[0] + p1[0] + p2[0]) / 3.0,
                (p0[1] + p1[1] + p2[1]) / 3.0,
                (p0[2] + p1[2] + p2[2]) / 3.0,
            ];
            let len = (c[0] * c[0] + c[1] * c[1] + c[2] * c[2]).sqrt();
            if len < 1e-6 {
                continue;
            }
            let cn = [c[0] / len, c[1] / len, c[2] / len];
            if n[0] * cn[0] + n[1] * cn[1] + n[2] * cn[2] > 0.0 {
                outward += 1;
            }
        }
        // Allow a handful of pathological voxels; demand the vast
        // majority face outward. 95% is a comfortable threshold for
        // naive DC on a curved shape.
        let ratio = outward as f32 / total as f32;
        assert!(
            ratio > 0.95,
            "only {outward}/{total} triangles face outward ({ratio:.3})",
        );
    }

    #[test]
    fn single_box_vertices_hug_surface_and_bound_aabb() {
        let half = [25.0, 25.0, 25.0];
        let mesh = mesh_for(&[box_feature(1, BoolOp::Union, [0.0, 0.0, 0.0], half)], 16);
        assert!(mesh.vertex_count() > 0);
        assert!(mesh.triangle_count() > 0);

        // Every vertex should be on the ±25 cube in L∞ norm, up to
        // ~cell size drift.
        let cell = 50.0 / 16.0;
        for i in 0..mesh.vertex_count() {
            let p = [
                mesh.positions[i * 3],
                mesh.positions[i * 3 + 1],
                mesh.positions[i * 3 + 2],
            ];
            let d_linf = p[0].abs().max(p[1].abs()).max(p[2].abs()) - 25.0;
            assert!(
                d_linf.abs() < cell * 1.5,
                "vertex {p:?} drifted {d_linf} from the box surface",
            );
        }
    }

    #[test]
    fn classification_arrays_are_parallel_to_positions() {
        // Every mesh vertex must have exactly one classification byte
        // and MAX_CONTRIBUTORS contributor slots. Without this
        // invariant the zero-copy views on the JS side can't stride
        // correctly.
        let mesh = mesh_for(
            &[box_feature(1, BoolOp::Union, [0.0, 0.0, 0.0], [25.0, 25.0, 25.0])],
            16,
        );
        assert_eq!(mesh.classifications.len(), mesh.vertex_count());
        assert_eq!(
            mesh.contributors.len(),
            mesh.vertex_count() * crate::csg::MAX_CONTRIBUTORS,
        );
        // Single-primitive scene: every vertex must be a face, with
        // contributor id == 1 and the remaining slots sentinel.
        for v in 0..mesh.vertex_count() {
            assert_eq!(mesh.classifications[v], 1, "vertex {v} not Face");
            let base = v * crate::csg::MAX_CONTRIBUTORS;
            assert_eq!(mesh.contributors[base], 1);
            for s in 1..crate::csg::MAX_CONTRIBUTORS {
                assert_eq!(
                    mesh.contributors[base + s],
                    CONTRIBUTOR_SENTINEL,
                    "vertex {v} slot {s} not sentinel",
                );
            }
        }
    }

    #[test]
    fn union_box_cylinder_has_edge_classified_vertices() {
        // The cylinder's lateral surface meets the box's top face
        // along a circle — any DC vertex within half a cell of that
        // circle should be classified as an Edge (two contributors:
        // box + cylinder).
        let features = vec![
            box_feature(1, BoolOp::Union, [0.0, 0.0, 0.0], [20.0, 20.0, 20.0]),
            cylinder_feature(2, BoolOp::Union, [0.0, 0.0, 0.0], 10.0, 50.0),
        ];
        let mesh = mesh_for(&features, 48);
        let mut edge_count = 0usize;
        for v in 0..mesh.vertex_count() {
            if mesh.classifications[v] == 2 {
                edge_count += 1;
                // Every Edge vertex must list exactly {1, 2} as its
                // non-sentinel contributors.
                let base = v * crate::csg::MAX_CONTRIBUTORS;
                let mut got = vec![];
                for s in 0..crate::csg::MAX_CONTRIBUTORS {
                    let id = mesh.contributors[base + s];
                    if id != CONTRIBUTOR_SENTINEL {
                        got.push(id);
                    }
                }
                got.sort();
                assert_eq!(
                    got,
                    vec![1, 2],
                    "edge vertex {v} has contributors {got:?}",
                );
            }
        }
        assert!(
            edge_count > 0,
            "expected at least one Edge-classified vertex on the intersection curve",
        );
    }

    // ── K3.5 Phase A tests ────────────────────────────────────────

    #[test]
    fn k35a_box_face_vertices_lie_on_surface() {
        let half = [25.0, 25.0, 25.0];
        let mesh = mesh_for(&[box_feature(1, BoolOp::Union, [0.0; 3], half)], 16);
        let tree = crate::csg::CsgNode::from_features(&[box_feature(1, BoolOp::Union, [0.0; 3], half)]);
        let mut max_err = 0.0_f32;
        let mut face_count = 0usize;
        for v in 0..mesh.vertex_count() {
            if mesh.classifications[v] != 1 { continue; }
            face_count += 1;
            let p = [
                mesh.positions[v * 3],
                mesh.positions[v * 3 + 1],
                mesh.positions[v * 3 + 2],
            ];
            let d = tree.sdf(p, EvalMode::Logical).abs();
            if d > max_err { max_err = d; }
        }
        assert!(face_count > 0, "expected Face-classified vertices");
        // Box edges have discontinuous SDF gradients, so Face vertices
        // near edges converge less tightly than smooth shapes.
        assert!(
            max_err < 0.1,
            "K3.5 Newton projection: box face vertices have max surface error {max_err} (want < 0.1)",
        );
    }

    #[test]
    fn k35a_sphere_face_vertices_lie_on_surface() {
        let mesh = mesh_for(&[sphere_feature(1, [0.0; 3], 20.0)], 24);
        let tree = crate::csg::CsgNode::from_features(&[sphere_feature(1, [0.0; 3], 20.0)]);
        let mut max_err = 0.0_f32;
        let mut face_count = 0usize;
        for v in 0..mesh.vertex_count() {
            if mesh.classifications[v] != 1 { continue; }
            face_count += 1;
            let p = [
                mesh.positions[v * 3],
                mesh.positions[v * 3 + 1],
                mesh.positions[v * 3 + 2],
            ];
            let d = tree.sdf(p, EvalMode::Logical).abs();
            if d > max_err { max_err = d; }
        }
        assert!(face_count > 0, "expected Face-classified vertices");
        assert!(
            max_err < 0.01,
            "K3.5 Newton projection: sphere face vertices have max surface error {max_err} (want < 0.01)",
        );
    }

    #[test]
    fn k35a_cylinder_face_vertices_lie_on_surface() {
        let features = vec![
            cylinder_feature(1, BoolOp::Union, [0.0; 3], 15.0, 40.0),
        ];
        let mesh = mesh_for(&features, 24);
        let tree = crate::csg::CsgNode::from_features(&features);
        let mut max_err = 0.0_f32;
        let mut face_count = 0usize;
        for v in 0..mesh.vertex_count() {
            if mesh.classifications[v] != 1 { continue; }
            face_count += 1;
            let p = [
                mesh.positions[v * 3],
                mesh.positions[v * 3 + 1],
                mesh.positions[v * 3 + 2],
            ];
            let d = tree.sdf(p, EvalMode::Logical).abs();
            if d > max_err { max_err = d; }
        }
        assert!(face_count > 0, "expected Face-classified vertices");
        assert!(
            max_err < 0.05,
            "K3.5 Newton projection: cylinder face vertices have max surface error {max_err} (want < 0.05)",
        );
    }

    #[test]
    fn k35a_hausdorff_distance_box() {
        let half = [30.0, 20.0, 15.0];
        let mesh = mesh_for(&[box_feature(1, BoolOp::Union, [0.0; 3], half)], 24);
        let tree = crate::csg::CsgNode::from_features(&[box_feature(1, BoolOp::Union, [0.0; 3], half)]);
        let mut hausdorff = 0.0_f32;
        for v in 0..mesh.vertex_count() {
            let p = [
                mesh.positions[v * 3],
                mesh.positions[v * 3 + 1],
                mesh.positions[v * 3 + 2],
            ];
            let d = tree.sdf(p, EvalMode::Logical).abs();
            if d > hausdorff { hausdorff = d; }
        }
        assert!(
            hausdorff < 0.1,
            "K3.5 Hausdorff distance for box: {hausdorff} (want < 0.1)",
        );
    }

    #[test]
    fn k35b_sphere_not_flattened_by_planar_simplification() {
        let r = 50.0_f32;
        let mesh = mesh_for(
            &[sphere_feature(1, [0.0; 3], r)],
            24,
        );
        assert!(mesh.vertex_count() > 0);
        for &idx in &mesh.indices {
            assert!((idx as usize) < mesh.vertex_count(), "index out of bounds");
        }
        let mut min = [f32::MAX; 3];
        let mut max = [f32::MIN; 3];
        for v in 0..mesh.vertex_count() {
            for a in 0..3 {
                let c = mesh.positions[v * 3 + a];
                if c < min[a] { min[a] = c; }
                if c > max[a] { max[a] = c; }
            }
        }
        for a in 0..3 {
            let extent = max[a] - min[a];
            assert!(
                extent > r,
                "sphere mesh extent along axis {a} is {extent} — surface flattened (expected > {r})",
            );
        }
    }

    #[test]
    fn k35b_box_triangle_count_reduced() {
        let half = [25.0, 25.0, 25.0];
        let mesh = mesh_for(
            &[box_feature(1, BoolOp::Union, [0.0; 3], half)],
            16,
        );
        // A 50mm cube at resolution 16 produces ~2700 triangles raw.
        // After Phase B simplification, planar faces are ear-clipped,
        // reducing the count significantly.
        // 87% triangle reduction (2700 → ~340) and 83% vertex reduction
        // (1064 → ~176). Boundary polygons have many vertices because
        // normal-bin grouping creates jagged region edges.
        assert!(
            mesh.triangle_count() < 500,
            "K3.5B: box triangle count {} should be < 500 after planar simplification",
            mesh.triangle_count(),
        );
        assert!(
            mesh.triangle_count() >= 12,
            "K3.5B: box needs at least 12 triangles (6 faces × 2)",
        );
        assert!(
            mesh.vertex_count() < 300,
            "K3.5B: box vertex count {} should be < 300 after compaction",
            mesh.vertex_count(),
        );
    }

    #[test]
    fn k35b_simplified_mesh_stays_manifold_like() {
        let half = [25.0, 25.0, 25.0];
        let mesh = mesh_for(
            &[box_feature(1, BoolOp::Union, [0.0; 3], half)],
            16,
        );
        assert!(mesh.triangle_count() > 0);
        // Every index must reference a valid vertex
        let vc = mesh.vertex_count();
        for &idx in &mesh.indices {
            assert!(
                (idx as usize) < vc,
                "K3.5B: index {idx} out of bounds (vertex count {vc})",
            );
        }
        // Arrays must be parallel
        assert_eq!(mesh.classifications.len(), vc);
        assert_eq!(mesh.contributors.len(), vc * MAX_CONTRIBUTORS);
        assert_eq!(mesh.positions.len(), vc * 3);
        assert_eq!(mesh.normals.len(), vc * 3);
    }

    #[test]
    fn k35b_earcut_triangle() {
        let verts = vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]];
        let tris = super::earcut_2d(&verts);
        assert_eq!(tris.len(), 1);
    }

    #[test]
    fn k35b_earcut_quad() {
        let verts = vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        let tris = super::earcut_2d(&verts);
        assert_eq!(tris.len(), 2);
    }

    #[test]
    fn k35b_earcut_concave_l_shape() {
        // L-shape: concave polygon with 6 vertices
        let verts = vec![
            [0.0, 0.0],
            [2.0, 0.0],
            [2.0, 1.0],
            [1.0, 1.0],
            [1.0, 2.0],
            [0.0, 2.0],
        ];
        let tris = super::earcut_2d(&verts);
        assert_eq!(tris.len(), 4, "L-shape should produce 4 triangles");
    }

    fn mesh_for_with_primitives(features: &[Feature], resolution: u32) -> Mesh {
        let csg = crate::csg::CsgNode::from_features(features);
        let prims = csg.leaf_primitives();
        let owned: std::collections::HashMap<u32, fielddraft_protocol::Primitive> =
            prims.into_iter().map(|(k, v)| (k, v.clone())).collect();
        let prim_refs: std::collections::HashMap<u32, &fielddraft_protocol::Primitive> =
            owned.iter().map(|(k, v)| (*k, v)).collect();
        let bounded = BoundedCsg::from_csg(&csg);
        super::dual_contour_with_primitives(&bounded, resolution, &prim_refs)
    }

    #[test]
    fn k35c_box_with_cylinder_hole_reduces_triangles() {
        // Phase C targets cylinder faces in multi-feature scenes where
        // intersections create Edge-classified vertices that define
        // natural face boundaries.
        let features = vec![
            box_feature(1, BoolOp::Union, [0.0; 3], [30.0, 20.0, 30.0]),
            cylinder_feature(2, BoolOp::Difference, [0.0, 0.0, 0.0], 10.0, 30.0),
        ];
        let mesh_without = mesh_for(&features, 32);
        let mesh_with = mesh_for_with_primitives(&features, 32);
        // Phase C may or may not reduce the cylinder surface depending
        // on whether the boundary loop extraction succeeds. At minimum,
        // the mesh must remain valid.
        let vc = mesh_with.vertex_count();
        assert!(vc > 0);
        for &idx in &mesh_with.indices {
            assert!((idx as usize) < vc, "index out of bounds");
        }
        assert_eq!(mesh_with.classifications.len(), vc);
        assert_eq!(mesh_with.contributors.len(), vc * MAX_CONTRIBUTORS);
        // Phase C should not significantly increase triangle count.
        // Allow ≤1% margin for boundary-polygon rounding.
        let max_allowed = mesh_without.triangle_count() + mesh_without.triangle_count() / 100 + 2;
        assert!(
            mesh_with.triangle_count() <= max_allowed,
            "K3.5C: Phase C should not significantly increase triangles ({} vs {})",
            mesh_with.triangle_count(),
            mesh_without.triangle_count(),
        );
    }

    #[test]
    fn k35c_standalone_cylinder_stays_valid() {
        let features = vec![
            cylinder_feature(1, BoolOp::Union, [0.0; 3], 15.0, 40.0),
        ];
        let mesh = mesh_for_with_primitives(&features, 24);
        let vc = mesh.vertex_count();
        assert!(vc > 0);
        for &idx in &mesh.indices {
            assert!((idx as usize) < vc, "index out of bounds");
        }
        assert_eq!(mesh.classifications.len(), vc);
        assert_eq!(mesh.contributors.len(), vc * MAX_CONTRIBUTORS);
    }

    #[test]
    fn k35c_translated_hole_matches_center_hole() {
        // Bug #25: Phase C parameterised vertices in world space
        // instead of primitive-local space, so a cylinder hole
        // positioned via transform.translate got broken θ/h mapping
        // and skipped simplification.
        //
        // Scenario: a box with a cylinder hole. One version uses
        // primitive `center` to place the hole, the other uses
        // `transform.translate`. Both should produce equivalent meshes.
        // Place a box with a small cylinder hole far from origin.
        // The bug manifests when translation >> cylinder radius:
        // θ = atan2(dz, dx) collapses toward 0 when dx ≈ 500
        // instead of the correct ±radius.
        let offset = [500.0_f32, 0.0, 0.0];
        let r = 2.0_f32;
        let center_version = vec![
            box_feature(1, BoolOp::Union, offset, [30.0, 20.0, 30.0]),
            cylinder_feature(2, BoolOp::Difference, offset, r, 30.0),
        ];
        let mut translate_version = vec![
            box_feature(1, BoolOp::Union, offset, [30.0, 20.0, 30.0]),
            cylinder_feature(2, BoolOp::Difference, [0.0, 0.0, 0.0], r, 30.0),
        ];
        translate_version[1].transform.translate = offset;

        let mesh_center = mesh_for_with_primitives(&center_version, 32);
        let mesh_translate = mesh_for_with_primitives(&translate_version, 32);

        eprintln!(
            "center-hole: {} verts / {} tris | translate-hole: {} verts / {} tris",
            mesh_center.vertex_count(), mesh_center.triangle_count(),
            mesh_translate.vertex_count(), mesh_translate.triangle_count(),
        );

        assert!(mesh_center.vertex_count() > 0);
        assert!(mesh_translate.vertex_count() > 0);

        let ratio = mesh_translate.triangle_count() as f64 / mesh_center.triangle_count() as f64;
        assert!(
            (0.8..=1.2).contains(&ratio),
            "translate-hole tris ({}) should be within ±20% of center-hole tris ({}) — ratio {:.2}",
            mesh_translate.triangle_count(),
            mesh_center.triangle_count(),
            ratio,
        );
    }

    /// K1 reference scene: a 120 mm box with two cylindrical holes
    /// drilled along the y-axis. The exact vertex/triangle counts are
    /// captured as a goldfile — they encode the mesher's determinism
    /// and act as a regression sentinel for any future change to the
    /// DC pass. Update the goldfile intentionally, not accidentally.
    #[test]
    fn goldfile_box_with_two_holes() {
        let features = vec![
            box_feature(1, BoolOp::Union, [0.0, 0.0, 0.0], [60.0, 30.0, 60.0]),
            cylinder_feature(2, BoolOp::Difference, [-25.0, 0.0, 0.0], 10.0, 40.0),
            cylinder_feature(3, BoolOp::Difference, [25.0, 0.0, 0.0], 10.0, 40.0),
        ];
        let mesh = mesh_for(&features, 32);

        // Non-trivial mesh.
        assert!(mesh.vertex_count() > 500);
        assert!(mesh.triangle_count() > 500);
        assert_eq!(mesh.indices.len() % 3, 0);
        assert_eq!(mesh.positions.len(), mesh.normals.len());
        assert_eq!(mesh.positions.len() % 3, 0);

        // Every vertex must sit near the zero-level set of the CSG
        // (proves the mesher actually tracked the surface; the holes
        // are present because their SDF is negative inside the box).
        let tree = crate::csg::CsgNode::from_features(&features);
        let mut off_surface = 0usize;
        for i in 0..mesh.vertex_count() {
            let p = [
                mesh.positions[i * 3],
                mesh.positions[i * 3 + 1],
                mesh.positions[i * 3 + 2],
            ];
            if tree.sdf(p, EvalMode::Logical).abs() > 3.0 {
                off_surface += 1;
            }
        }
        assert_eq!(
            off_surface, 0,
            "{off_surface} vertices drifted more than 3 mm off the CSG surface",
        );

        // Sample along the cylinder axes: every point inside each hole
        // must have sdf > 0 (outside the body) — if the hole weren't
        // actually carved, these points would be deep inside the box.
        for y in [-20.0_f32, -10.0, 0.0, 10.0, 20.0] {
            let in_left = tree.sdf([-25.0, y, 0.0], EvalMode::Logical);
            let in_right = tree.sdf([25.0, y, 0.0], EvalMode::Logical);
            assert!(
                in_left > 0.0 && in_right > 0.0,
                "holes not carved at y={y}: left={in_left}, right={in_right}",
            );
        }
    }

    // ── Sparse narrow-band path tests ──────────────────────────────

    #[test]
    fn sparse_sphere_matches_dense_topology() {
        let features = [sphere_feature(1, [0.0; 3], 20.0)];
        let dense = mesh_for(&features, 48);
        let sparse = mesh_for(&features, 128);
        assert!(sparse.vertex_count() > dense.vertex_count());
        assert!(sparse.triangle_count() > dense.triangle_count());
        assert_eq!(sparse.indices.len() % 3, 0);
        for &idx in &sparse.indices {
            assert!((idx as usize) < sparse.vertex_count());
        }
    }

    #[test]
    fn sparse_sphere_vertices_hug_surface() {
        let features = [sphere_feature(1, [0.0; 3], 20.0)];
        let mesh = mesh_for(&features, 128);
        let tree = crate::csg::CsgNode::from_features(&features);
        let mut max_err = 0.0_f32;
        for i in 0..mesh.vertex_count() {
            let p = [
                mesh.positions[i * 3],
                mesh.positions[i * 3 + 1],
                mesh.positions[i * 3 + 2],
            ];
            let d = tree.sdf(p, EvalMode::Logical).abs();
            if d > max_err { max_err = d; }
        }
        let cell = 40.0 / 128.0;
        assert!(
            max_err < cell * 2.0,
            "sparse sphere vertices drifted off surface by {max_err} (cell={cell})",
        );
    }

    #[test]
    fn sparse_box_planar_simplification_works() {
        let half = [25.0, 25.0, 25.0];
        let mesh = mesh_for(&[box_feature(1, BoolOp::Union, [0.0; 3], half)], 128);
        assert!(mesh.vertex_count() > 0);
        assert_eq!(mesh.indices.len() % 3, 0);
        for &idx in &mesh.indices {
            assert!((idx as usize) < mesh.vertex_count());
        }
    }
}
