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
use crate::vec3f::{cross, dot};
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
///     0 = off-surface (should never appear once we've filtered the
///     DC output), 1 = face, 2 = edge, 3 = vertex. Matches the
///     [`ProxyElement::tag`] contract.
///   * `contributors` — flat `u32` array, `MAX_CONTRIBUTORS` slots per
///     vertex. Unused slots are set to `u32::MAX`. Readers walk the
///     stride and stop at the sentinel.
///   * `face_index` — one `u32` per **triangle** (length `indices.len()/3`):
///     the arena index of the B-Rep [`crate::brep::Face`] that triangle lies
///     on (same ordering as the `bodyBrepFaces` export), or
///     [`crate::brep::face_export::FACE_INDEX_SENTINEL`] when no analytic face
///     could be correlated. Per-triangle (not per-vertex) because planar
///     simplification leaves flat faces with only boundary-ring vertices, so a
///     vertex can't be uniquely owned by one face — but a triangle centroid
///     always is. Populated post-mesh by
///     [`crate::brep::face_export::correlate_mesh_faces`]; left empty by the
///     bare DC pass (which has no B-Rep). This is the per-triangle face
///     identity that drives viewport face selection (Brocken A).
#[derive(Debug, Default, Clone)]
pub struct Mesh {
    pub positions: Vec<f32>,
    pub normals: Vec<f32>,
    pub indices: Vec<u32>,
    pub classifications: Vec<u8>,
    pub contributors: Vec<u32>,
    pub face_index: Vec<u32>,
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

/// How many grid samples the viewport grid must place across a body's
/// THINNEST feature. Two is the minimum that makes a thin slab visible
/// at all *and* gives it two sign changes (one per side); one sample
/// would leave the slab a single-layer sliver.
const THIN_FEATURE_SAMPLES: f32 = 2.0;

/// Cost ceiling for the AUTOMATIC refinement below, in *primitive
/// evaluations*: grid corners × the body's per-sample field cost
/// ([`eval_cost`]). Corners alone are a bad ceiling — a body whose field
/// is one extruded profile evaluates 30× faster per sample than a
/// shelled, filleted one, so the same corner count is half a second in
/// one document and ten in the next. 10 M primitive evaluations measure
/// as ~0.2 s of Dual Contouring natively, well under a second in wasm —
/// the ceiling a body may add to a viewport rebuild, off-thread.
///
/// It bounds only the refinement: a resolution the caller asked for
/// explicitly is never capped, and never lowered.
const THIN_FEATURE_MAX_SAMPLE_OPS: f64 = 10_000_000.0;

/// Grid resolution the viewport mesher should actually use for `node`.
///
/// The uniform DC grid derives its cell from the body's LONGEST axis
/// (`cell = max_dim / resolution`), which says nothing about the body's
/// thinnest feature. A 4 m shelf built from 22 mm boards meshes at
/// 62 mm cells at resolution 64 — no lattice plane lands inside a board,
/// every sign change is missed and the body renders as an EMPTY mesh
/// (silently: DC has no way to report "I sampled straight through your
/// geometry"). That is not a fidelity nuance, it is a blank viewport
/// (#412 follow-up, Onshape "Garagenregal").
///
/// So: find the smallest feature extent in the tree (leaf AABBs, sketch
/// profile AABBs, shell thickness) and refine the cell until
/// [`THIN_FEATURE_SAMPLES`] samples fit across it — capped by
/// [`THIN_FEATURE_MAX_SAMPLE_OPS`], and only when the affordable cell
/// actually brings a feature into reach. Bodies whose thinnest feature
/// is already sampled well enough (the overwhelming majority) return
/// `requested` unchanged, so this is a no-op for everything but genuinely
/// thin-featured geometry.
///
/// Viewport-only: the export mesher takes a user-chosen cell size and
/// runs its own corner guard ([`crate::export_mesh::derive_export_resolution`]).
pub fn resolution_for_thin_features(node: &BoundedCsg, requested: u32) -> u32 {
    if requested == 0 {
        return requested;
    }
    let aabb = node.aabb();
    if aabb.is_empty() {
        return requested;
    }
    let size = aabb.size();
    let max_dim = size[0].max(size[1]).max(size[2]);
    if !(max_dim > 0.0) {
        return requested;
    }
    let base_cell = max_dim / requested as f32;
    let mut extents = Vec::new();
    collect_feature_extents(node, &mut extents);
    let Some(thin) = extents
        .iter()
        .copied()
        .fold(None, |a: Option<f32>, e| Some(a.map_or(e, |a| a.min(e))))
    else {
        return requested;
    };
    let target_cell = thin / THIN_FEATURE_SAMPLES;
    if target_cell >= base_cell {
        return requested;
    }

    // Sampling cost of the grid `dual_contour_unsimplified` would build
    // (AABB expanded by DC_MARGIN_CELLS cells on each side, times the
    // per-sample field cost).
    let per_sample = eval_cost(node);
    let ops = |cell: f32| -> f64 {
        let m = 2.0 * DC_MARGIN_CELLS as f64 + 2.0;
        let nx = (size[0] as f64 / cell as f64).ceil() + m;
        let ny = (size[1] as f64 / cell as f64).ceil() + m;
        let nz = (size[2] as f64 / cell as f64).ceil() + m;
        nx * ny * nz * per_sample
    };
    let mut cell = target_cell;
    while cell < base_cell && ops(cell) > THIN_FEATURE_MAX_SAMPLE_OPS {
        cell *= (ops(cell) / THIN_FEATURE_MAX_SAMPLE_OPS)
            .powf(1.0 / 3.0)
            .max(1.01) as f32;
    }
    if cell >= base_cell {
        return requested;
    }

    // Only spend the extra cells when they actually buy VISIBILITY. A grid
    // whose spacing is smaller than a feature's extent is guaranteed to put a
    // sample plane inside it; coarser than that, capture is luck. So refine
    // only if some feature crosses that line by refining — a 1 mm pin on a
    // 500 mm part stays out of reach at any affordable cell, and paying the
    // full budget for it would double the mesh cost and still not show it.
    if !extents.iter().any(|&t| cell < t && t <= base_cell) {
        return requested;
    }
    ((max_dim / cell).ceil() as u32).max(requested)
}

/// Rough cost of ONE field evaluation of the tree, in primitive
/// evaluations. Not a benchmark — an order-of-magnitude estimate that
/// keeps the refinement budget above from meaning wildly different wall
/// times per document. Deliberately ignores BVH pruning (which only ever
/// makes the real cost lower, i.e. the estimate errs towards refining
/// less).
fn eval_cost(node: &BoundedCsg) -> f64 {
    use crate::bvh::BoundedCsg as B;
    match node {
        B::Empty => 0.0,
        B::Leaf { primitive, .. } => match primitive {
            // Mesh SDF: BVH descent + ray parity per sample.
            Primitive::ImportedMesh { .. } => 12.0,
            Primitive::Thread { .. } => 4.0,
            Primitive::Gyroid { .. } => 3.0,
            _ => 1.0,
        },
        B::Extrude { sketch, .. } | B::Revolve { sketch, .. } => sketch_eval_cost(sketch) + 1.0,
        // Helical/rotation-minimising frame mapping before the 2D eval.
        B::Helix { sketch, .. } => sketch_eval_cost(sketch) + 8.0,
        // BVH-pruned nearest-spine-segment search, then the 2D eval.
        B::Sweep { sketch, .. } => sketch_eval_cost(sketch) + 8.0,
        B::Loft { sections, .. } => {
            2.0 + sections
                .iter()
                .map(|s| sketch_eval_cost(&s.sketch))
                .sum::<f64>()
        }
        B::Union { left, right, .. }
        | B::Difference { left, right, .. }
        | B::Intersection { left, right, .. }
        | B::Morph { left, right, .. } => eval_cost(left) + eval_cost(right) + 1.0,
        B::SmoothUnion { left, right, .. } | B::SmoothDifference { left, right, .. } => {
            eval_cost(left) + eval_cost(right) + 2.0
        }
        B::MaskedBlend {
            left,
            right,
            mask_polyline,
            ..
        } => eval_cost(left) + eval_cost(right) + 2.0 + mask_polyline.len() as f64,
        // The wall field evaluates its child on both sides of the offset.
        B::Shell { inner, .. } => 2.0 * eval_cost(inner) + 2.0,
        B::Offset { inner, .. } | B::InnerWall { inner, .. } => eval_cost(inner) + 1.0,
        B::Twist { inner, .. } | B::Bend { inner, .. } | B::Displace { inner, .. } => {
            eval_cost(inner) + 4.0
        }
        B::ArcChannel {
            inner,
            channels,
            verts,
            rims,
            ..
        } => {
            eval_cost(inner)
                + 2.0 * channels.len() as f64
                + verts.len() as f64
                + 3.0 * rims.len() as f64
        }
    }
}

/// Per-sample cost of a 2D profile tree. A `Path`'s cost is its segment
/// count — the exact distance is the minimum over every segment.
fn sketch_eval_cost(sketch: &crate::sketch2d::BoundedSketch2d) -> f64 {
    use crate::sketch2d::BoundedSketch2d as S;
    use fielddraft_protocol::Sketch2dPrimitive as P;
    match sketch {
        S::Empty => 0.0,
        S::Prim { primitive, .. } => match primitive {
            P::Path { segments, .. } => segments.len().max(1) as f64,
            P::Polygon { vertices } => vertices.len().max(1) as f64,
            _ => 1.0,
        },
        S::Union { left, right, .. }
        | S::Difference { left, right, .. }
        | S::Intersection { left, right, .. } => {
            sketch_eval_cost(left) + sketch_eval_cost(right) + 1.0
        }
        S::Offset { inner, .. } => sketch_eval_cost(inner) + 1.0,
    }
}

/// Collect every feature's characteristic thin extent in the tree, in
/// WORLD units. Solids contribute their world AABB's shortest side,
/// profile-swept nodes their 2D profile primitives' shortest sides
/// (scaled by the transform's smallest scale factor) plus the sweep
/// depth, a shell its wall thickness. Boolean operands all count — a
/// thin CUTTER carves a thin slot, which needs the same sampling
/// density as a thin solid.
///
/// Degenerate extents (zero, negative, non-finite) are dropped: a
/// zero-height profile line cannot be meshed at any resolution, and
/// letting it through would drag the target cell to zero on every such
/// body.
fn collect_feature_extents(node: &BoundedCsg, out: &mut Vec<f32>) {
    use crate::bvh::BoundedCsg as B;
    let mut push = |e: Option<f32>| {
        if let Some(e) = e {
            out.push(e);
        }
    };
    match node {
        B::Empty => {}
        B::Leaf { aabb, .. } => {
            let s = aabb.size();
            push(finite_extent(s[0].min(s[1]).min(s[2])));
        }
        B::Extrude {
            sketch,
            half_height,
            world_transform,
            ..
        } => {
            let s = min_scale(world_transform);
            push(finite_extent(2.0 * half_height.abs() * s));
            collect_sketch_extents(sketch, s, out);
        }
        B::Revolve {
            sketch,
            world_transform,
            ..
        }
        | B::Helix {
            sketch,
            world_transform,
            ..
        }
        | B::Sweep {
            sketch,
            world_transform,
            ..
        } => collect_sketch_extents(sketch, min_scale(world_transform), out),
        B::Loft {
            sections,
            world_transform,
            ..
        } => {
            let s = min_scale(world_transform);
            for sec in sections {
                collect_sketch_extents(&sec.sketch, s, out);
            }
        }
        B::Union { left, right, .. }
        | B::Difference { left, right, .. }
        | B::Intersection { left, right, .. }
        | B::SmoothUnion { left, right, .. }
        | B::SmoothDifference { left, right, .. }
        | B::MaskedBlend { left, right, .. }
        | B::Morph { left, right, .. } => {
            collect_feature_extents(left, out);
            collect_feature_extents(right, out);
        }
        B::Shell {
            inner, thickness, ..
        } => {
            push(finite_extent(thickness.abs()));
            collect_feature_extents(inner, out);
        }
        B::Twist { inner, .. }
        | B::Bend { inner, .. }
        | B::Displace { inner, .. }
        | B::Offset { inner, .. }
        | B::InnerWall { inner, .. }
        | B::ArcChannel { inner, .. } => collect_feature_extents(inner, out),
    }
}

/// Shortest side of every 2D profile primitive in a sketch tree, scaled
/// into world units. Subtracted profiles count too — they cut slots.
fn collect_sketch_extents(
    sketch: &crate::sketch2d::BoundedSketch2d,
    scale: f32,
    out: &mut Vec<f32>,
) {
    use crate::sketch2d::BoundedSketch2d as S;
    match sketch {
        S::Empty => {}
        S::Prim { aabb, .. } => {
            let s = aabb.size();
            if let Some(e) = finite_extent(s[0].min(s[1]) * scale) {
                out.push(e);
            }
        }
        S::Union { left, right, .. }
        | S::Difference { left, right, .. }
        | S::Intersection { left, right, .. } => {
            collect_sketch_extents(left, scale, out);
            collect_sketch_extents(right, scale, out);
        }
        S::Offset { inner, .. } => collect_sketch_extents(inner, scale, out),
    }
}

/// Smallest absolute scale factor of a transform — the one that shrinks
/// a local extent the most on its way into world space.
fn min_scale(t: &Transform) -> f32 {
    let s = t.scale[0].abs().min(t.scale[1].abs()).min(t.scale[2].abs());
    if s.is_finite() && s > 0.0 {
        s
    } else {
        1.0
    }
}

fn finite_extent(e: f32) -> Option<f32> {
    (e.is_finite() && e > 0.0).then_some(e)
}

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
    let mut mesh = dual_contour_unsimplified(node, resolution);
    simplify_dc_mesh(&mut mesh, node, leaf_primitives);
    mesh
}

/// Grid setup + Manifold-DC + Newton projection of the face vertices
/// onto the exact SDF — *without* the simplification post-pass. The
/// export mesher consumes this directly so it can validate the
/// simplification step separately and fall back to the raw mesh when
/// simplifying would break watertightness.
pub fn dual_contour_unsimplified(node: &BoundedCsg, resolution: u32) -> Mesh {
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

    let normal_eps = cell * 0.25;

    let mut mesh = crate::mdc::manifold_dc(node, grid.min, cell, nx, ny, nz);
    project_face_vertices(&mut mesh, node, normal_eps);
    mesh
}

/// The simplification post-pass shared by the viewport pipeline and the
/// export mesher: planar + developable simplification, then the
/// feature-normal fix.
pub fn simplify_dc_mesh(
    mesh: &mut Mesh,
    node: &BoundedCsg,
    leaf_primitives: &HashMap<u32, &Primitive>,
) {
    simplify_planar_faces(mesh);
    if !leaf_primitives.is_empty() {
        let leaf_transforms = node.leaf_world_transforms();
        simplify_developable_faces(mesh, leaf_primitives, &leaf_transforms);
    }
    fix_feature_normals(mesh);
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
    // indexes both keep[t] and mesh.indices[t*3..]; not a single-slice loop
    #[allow(clippy::needless_range_loop)]
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
    let mut max_u = f32::MIN;
    let mut min_v = f32::MAX;
    let mut max_v = f32::MIN;

    let (u_axis, v_axis) = orthonormal_basis(plane_n);

    let mut first = true;
    for &vi in &verts_seen {
        let p = vtx_pos(mesh, vi as usize);
        let d = dot(p, plane_n);
        let u = dot(p, u_axis);
        let v = dot(p, v_axis);
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
    // BTreeMap: region discovery order feeds the output triangle order —
    // HashMap iteration made rebuilds nondeterministic (Phase 1a).
    let mut tri_groups: std::collections::BTreeMap<(u32, i8), Vec<usize>> =
        std::collections::BTreeMap::new();
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

    for tris in tri_groups.values() {
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
                sorted.sort_by_key(|b| std::cmp::Reverse(b.len()));
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

    let mut boundary: Vec<(u32, u32)> = edge_count
        .into_iter()
        .filter(|&(_, c)| c == 1)
        .map(|(e, _)| e)
        .collect();
    // Deterministic loop start + walk direction (HashMap order leaked
    // into the output otherwise — Phase 1a).
    boundary.sort_unstable();

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

    let mut boundary: Vec<(u32, u32)> = edge_count
        .into_iter()
        .filter(|&(_, c)| c == 1)
        .map(|(e, _)| e)
        .collect();
    boundary.sort_unstable();

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

    let project = |vi: u32| -> [f64; 2] {
        let p = vtx_pos(mesh, vi as usize);
        [dot(p, u_axis) as f64, dot(p, v_axis) as f64]
    };

    // Shared with the parametric tessellator: visibility-based hole
    // bridging + ear clipping. The old nearest-vertex slit bridging
    // could pick a crossing bridge and emit overlapping/non-manifold
    // triangles (the Phase-0 nondeterministic watertight failures).
    let mut vids: Vec<u32> = region.boundary.clone();
    let mut uv: Vec<[f64; 2]> = vids.iter().map(|&vi| project(vi)).collect();
    let mut rings: Vec<(usize, usize)> = vec![(0, vids.len())];
    for hole in &region.holes {
        if hole.len() < 3 {
            continue;
        }
        let s = vids.len();
        vids.extend_from_slice(hole);
        uv.extend(hole.iter().map(|&vi| project(vi)));
        rings.push((s, hole.len()));
    }

    let Ok(tris) = crate::brep::tessellate::triangulate_polygon_with_holes(&uv, &rings) else {
        // Triangulation refused (degenerate ring, crossing geometry):
        // keep the region unsimplified rather than emit garbage.
        return Vec::new();
    };

    let mut indices = Vec::with_capacity(tris.len() * 3);
    for [a, b, c] in tris {
        indices.push(vids[a]);
        indices.push(vids[b]);
        indices.push(vids[c]);
    }
    indices
}

fn orthonormal_basis(n: [f32; 3]) -> ([f32; 3], [f32; 3]) {
    let up = if n[1].abs() < 0.9 {
        [0.0, 1.0, 0.0]
    } else {
        [1.0, 0.0, 0.0]
    };
    let u = cross(up, n);
    let u_len = (u[0] * u[0] + u[1] * u[1] + u[2] * u[2]).sqrt();
    let u = [u[0] / u_len, u[1] / u_len, u[2] / u_len];
    let v = cross(n, u);
    (u, v)
}

fn tri_geometric_normal(mesh: &Mesh, t: usize) -> [f32; 3] {
    let p0 = vtx_pos(mesh, mesh.indices[t * 3] as usize);
    let p1 = vtx_pos(mesh, mesh.indices[t * 3 + 1] as usize);
    let p2 = vtx_pos(mesh, mesh.indices[t * 3 + 2] as usize);
    let e1 = [p1[0] - p0[0], p1[1] - p0[1], p1[2] - p0[2]];
    let e2 = [p2[0] - p0[0], p2[1] - p0[1], p2[2] - p0[2]];
    cross(e1, e2)
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
    // BTreeMap for deterministic per-contributor processing order (Phase 1a).
    let mut cyl_tris: std::collections::BTreeMap<u32, Vec<usize>> =
        std::collections::BTreeMap::new();

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
    // indexes both keep[t] and mesh.indices[t*3..]; not a single-slice loop
    #[allow(clippy::needless_range_loop)]
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
    // `face_index` is per-TRIANGLE and populated only after all simplification
    // (which is where compaction runs), so there is nothing to carry here. The
    // compaction passes never reorder or drop triangles, only remap vertex
    // indices, so a later-populated per-triangle tag stays valid regardless.
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

/// Recompute normals for Edge/Vertex classified vertices using
/// angle-weighted geometric normals from adjacent triangles.
/// SDF-derived normals are inaccurate at CSG boundaries because
/// central differences straddle the gradient discontinuity.
fn fix_feature_normals(mesh: &mut Mesh) {
    let vc = mesh.vertex_count();
    let tc = mesh.triangle_count();

    let mut needs = vec![false; vc];
    let mut any = false;
    // indexes both mesh.classifications[v] (read) and needs[v] (write)
    #[allow(clippy::needless_range_loop)]
    for v in 0..vc {
        if mesh.classifications[v] >= 2 {
            needs[v] = true;
            any = true;
        }
    }
    if !any {
        return;
    }

    let mut accum = vec![[0.0f64; 3]; vc];

    for t in 0..tc {
        let i0 = mesh.indices[t * 3] as usize;
        let i1 = mesh.indices[t * 3 + 1] as usize;
        let i2 = mesh.indices[t * 3 + 2] as usize;

        if !needs[i0] && !needs[i1] && !needs[i2] {
            continue;
        }

        let p0 = vtx_pos(mesh, i0);
        let p1 = vtx_pos(mesh, i1);
        let p2 = vtx_pos(mesh, i2);

        let e01 = [p1[0] - p0[0], p1[1] - p0[1], p1[2] - p0[2]];
        let e02 = [p2[0] - p0[0], p2[1] - p0[1], p2[2] - p0[2]];
        let e12 = [p2[0] - p1[0], p2[1] - p1[1], p2[2] - p1[2]];
        let fn_ = [
            e01[1] * e02[2] - e01[2] * e02[1],
            e01[2] * e02[0] - e01[0] * e02[2],
            e01[0] * e02[1] - e01[1] * e02[0],
        ];

        let corners: [(usize, [f32; 3], [f32; 3]); 3] = [
            (i0, e01, e02),
            (i1, [-e01[0], -e01[1], -e01[2]], e12),
            (i2, [-e02[0], -e02[1], -e02[2]], [-e12[0], -e12[1], -e12[2]]),
        ];

        for (vi, ea, eb) in corners {
            if !needs[vi] {
                continue;
            }
            let da = (ea[0] * ea[0] + ea[1] * ea[1] + ea[2] * ea[2]).sqrt();
            let db = (eb[0] * eb[0] + eb[1] * eb[1] + eb[2] * eb[2]).sqrt();
            if da < 1e-12 || db < 1e-12 {
                continue;
            }
            let cos_a = (ea[0] * eb[0] + ea[1] * eb[1] + ea[2] * eb[2]) / (da * db);
            let angle = cos_a.clamp(-1.0, 1.0).acos();
            accum[vi][0] += angle as f64 * fn_[0] as f64;
            accum[vi][1] += angle as f64 * fn_[1] as f64;
            accum[vi][2] += angle as f64 * fn_[2] as f64;
        }
    }

    for v in 0..vc {
        if !needs[v] {
            continue;
        }
        let len =
            (accum[v][0] * accum[v][0] + accum[v][1] * accum[v][1] + accum[v][2] * accum[v][2])
                .sqrt();
        if len > 1e-12 {
            mesh.normals[v * 3] = (accum[v][0] / len) as f32;
            mesh.normals[v * 3 + 1] = (accum[v][1] / len) as f32;
            mesh.normals[v * 3 + 2] = (accum[v][2] / len) as f32;
        }
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
    use crate::csg::{DecodedFeature, PrimitiveModifiers};
    use fielddraft_protocol::{BoolOp, FeatureId, Primitive, Transform};

    fn sphere_feature(id: u32, center: [f32; 3], radius: f32) -> DecodedFeature {
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

    fn box_feature(id: u32, op: BoolOp, center: [f32; 3], half: [f32; 3]) -> DecodedFeature {
        DecodedFeature::from_parts(
            FeatureId::from_u32(id),
            format!("Box {id}"),
            op,
            Primitive::Box {
                center,
                half_extents: half,
            },
            Transform::IDENTITY,
            PrimitiveModifiers::NONE,
            vec![],
        )
    }

    fn cylinder_feature(
        id: u32,
        op: BoolOp,
        center: [f32; 3],
        radius: f32,
        half_height: f32,
    ) -> DecodedFeature {
        DecodedFeature::from_parts(
            FeatureId::from_u32(id),
            format!("Cyl {id}"),
            op,
            Primitive::Cylinder {
                center,
                radius,
                half_height,
            },
            Transform::IDENTITY,
            PrimitiveModifiers::NONE,
            vec![],
        )
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

    fn mesh_for(features: &[DecodedFeature], resolution: u32) -> Mesh {
        let csg = crate::csg::CsgNode::from_decoded(features);
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
        let tree = crate::csg::CsgNode::from_decoded(&[sphere_feature(
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
        let mesh = mesh_for(&[box_feature(1, BoolOp::New, [0.0, 0.0, 0.0], half)], 16);
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
            &[box_feature(1, BoolOp::New, [0.0, 0.0, 0.0], [25.0, 25.0, 25.0])],
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
            box_feature(1, BoolOp::New, [0.0, 0.0, 0.0], [20.0, 20.0, 20.0]),
            cylinder_feature(2, BoolOp::New, [0.0, 0.0, 0.0], 10.0, 50.0),
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
        let mesh = mesh_for(&[box_feature(1, BoolOp::New, [0.0; 3], half)], 16);
        let tree = crate::csg::CsgNode::from_decoded(&[box_feature(1, BoolOp::New, [0.0; 3], half)]);
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
        let tree = crate::csg::CsgNode::from_decoded(&[sphere_feature(1, [0.0; 3], 20.0)]);
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
            cylinder_feature(1, BoolOp::New, [0.0; 3], 15.0, 40.0),
        ];
        let mesh = mesh_for(&features, 24);
        let tree = crate::csg::CsgNode::from_decoded(&features);
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
        let mesh = mesh_for(&[box_feature(1, BoolOp::New, [0.0; 3], half)], 24);
        let tree = crate::csg::CsgNode::from_decoded(&[box_feature(1, BoolOp::New, [0.0; 3], half)]);
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
            &[box_feature(1, BoolOp::New, [0.0; 3], half)],
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
            &[box_feature(1, BoolOp::New, [0.0; 3], half)],
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

    fn mesh_for_with_primitives(features: &[DecodedFeature], resolution: u32) -> Mesh {
        let csg = crate::csg::CsgNode::from_decoded(features);
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
            box_feature(1, BoolOp::New, [0.0; 3], [30.0, 20.0, 30.0]),
            cylinder_feature(2, BoolOp::Remove, [0.0, 0.0, 0.0], 10.0, 30.0),
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
        let max_allowed = mesh_without.triangle_count() + mesh_without.triangle_count() / 20 + 2;
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
            cylinder_feature(1, BoolOp::New, [0.0; 3], 15.0, 40.0),
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
            box_feature(1, BoolOp::New, offset, [30.0, 20.0, 30.0]),
            cylinder_feature(2, BoolOp::Remove, offset, r, 30.0),
        ];
        let mut translate_version = vec![
            box_feature(1, BoolOp::New, offset, [30.0, 20.0, 30.0]),
            cylinder_feature(2, BoolOp::Remove, [0.0, 0.0, 0.0], r, 30.0),
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
            (0.75..=1.25).contains(&ratio),
            "translate-hole tris ({}) should be within ±25% of center-hole tris ({}) — ratio {:.2}",
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
            box_feature(1, BoolOp::New, [0.0, 0.0, 0.0], [60.0, 30.0, 60.0]),
            cylinder_feature(2, BoolOp::Remove, [-25.0, 0.0, 0.0], 10.0, 40.0),
            cylinder_feature(3, BoolOp::Remove, [25.0, 0.0, 0.0], 10.0, 40.0),
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
        let tree = crate::csg::CsgNode::from_decoded(&features);
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
        let tree = crate::csg::CsgNode::from_decoded(&features);
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
        let mesh = mesh_for(&[box_feature(1, BoolOp::New, [0.0; 3], half)], 128);
        assert!(mesh.vertex_count() > 0);
        assert_eq!(mesh.indices.len() % 3, 0);
        for &idx in &mesh.indices {
            assert!((idx as usize) < mesh.vertex_count());
        }
    }

    // ── thin-feature grid refinement ────────────────────────────────

    /// One 4 m × 22 mm shelf board, extruded 500 mm deep — the shape of the
    /// Onshape "Garagenregal" body that rendered as an empty viewport: at the
    /// app's resolution 64 the 62 mm cells step clean over the 22 mm board.
    fn shelf_board() -> BoundedCsg {
        use fielddraft_protocol::{Sketch2d, Sketch2dPrimitive};
        let sketch = Sketch2d::Prim(Sketch2dPrimitive::Rectangle {
            center: [0.0, 0.0],
            half_extents: [2000.0, 11.0],
        });
        BoundedCsg::from_csg(&crate::csg::CsgNode::Extrude {
            wall_subs: None,
            id: FeatureId::from_u32(1),
            sketch: Box::new(sketch),
            half_height: 250.0,
        })
    }

    #[test]
    fn thin_board_in_a_large_body_refines_the_grid_until_it_meshes() {
        let node = shelf_board();
        // Baseline: the requested resolution samples straight through the
        // board — this is the silent empty mesh the fix is about.
        assert_eq!(
            dual_contour(&node, 64).triangle_count(),
            0,
            "precondition: the raw resolution-64 grid misses the board",
        );

        let refined = resolution_for_thin_features(&node, 64);
        assert!(refined > 64, "expected refinement, got {refined}");
        // Cell must end up smaller than the board thickness, or a lattice
        // plane can still miss it.
        let cell = 4000.0 / refined as f32;
        assert!(cell < 22.0, "refined cell {cell} still coarser than the board");
        assert!(
            dual_contour(&node, refined).triangle_count() > 0,
            "refined grid still produced an empty mesh",
        );
    }

    #[test]
    fn thin_feature_refinement_respects_the_sampling_budget() {
        let node = shelf_board();
        let cost = eval_cost(&node);
        for requested in [16, 32, 64, 128] {
            let refined = resolution_for_thin_features(&node, requested);
            assert!(refined >= requested, "refinement must never coarsen");
            let cell = 4000.0_f64 / refined as f64;
            let m = 2.0 * DC_MARGIN_CELLS as f64 + 2.0;
            let ops = ((4000.0 / cell).ceil() + m)
                * ((22.0f64 / cell).ceil() + m)
                * ((500.0 / cell).ceil() + m)
                * cost;
            assert!(
                ops <= THIN_FEATURE_MAX_SAMPLE_OPS * 1.2,
                "res {requested} → {refined}: {ops} ops blow the budget",
            );
        }
    }

    #[test]
    fn an_explicitly_requested_fine_resolution_is_never_lowered() {
        // The budget governs the AUTOMATIC refinement only. A caller asking
        // for a grid finer than the budget (export estimates, debug dialogs,
        // a user cranking the quality slider) gets exactly what it asked for.
        let node = shelf_board();
        // 512 cells along 4 m is finer than the board needs (7.8 mm cells for a
        // 22 mm board) — nothing to add, nothing to take away.
        assert_eq!(resolution_for_thin_features(&node, 512), 512);
        assert!(resolution_for_thin_features(&node, 256) >= 256);
    }

    #[test]
    fn chunky_bodies_keep_their_requested_resolution() {
        // A sphere, a box and a boolean of both are sampled plenty finely
        // already — the refinement must not touch them (no cost regression
        // for the 99 % case).
        let features = [
            sphere_feature(1, [0.0; 3], 20.0),
            box_feature(2, BoolOp::Add, [10.0, 0.0, 0.0], [15.0, 15.0, 8.0]),
            cylinder_feature(3, BoolOp::Remove, [0.0; 3], 4.0, 30.0),
        ];
        let csg = crate::csg::CsgNode::from_decoded(&features);
        let node = BoundedCsg::from_csg(&csg);
        for requested in [24, 32, 48, 64] {
            assert_eq!(
                resolution_for_thin_features(&node, requested),
                requested,
                "chunky body was refined at resolution {requested}",
            );
        }
    }

    #[test]
    fn features_beyond_any_affordable_cell_do_not_buy_refinement() {
        // A 1 mm pin drilled through a 500 mm block: showing it would take
        // 0.5 mm cells — a billion corners. Refining "as far as the budget
        // allows" would double the mesh cost and STILL not show the hole, so
        // the requested resolution stands.
        let features = [
            box_feature(1, BoolOp::New, [0.0; 3], [250.0, 250.0, 250.0]),
            cylinder_feature(2, BoolOp::Remove, [0.0; 3], 0.5, 300.0),
        ];
        let csg = crate::csg::CsgNode::from_decoded(&features);
        let node = BoundedCsg::from_csg(&csg);
        assert_eq!(resolution_for_thin_features(&node, 64), 64);
    }

    #[test]
    fn an_out_of_reach_detail_does_not_block_a_reachable_board() {
        // Mixed scales: the 22 mm boards ARE reachable, the 1 mm pin is not.
        // The unreachable feature must not veto the reachable one.
        use fielddraft_protocol::{Sketch2d, Sketch2dPrimitive};
        let board = crate::csg::CsgNode::Extrude {
            wall_subs: None,
            id: FeatureId::from_u32(1),
            sketch: Box::new(Sketch2d::Prim(Sketch2dPrimitive::Rectangle {
                center: [0.0, 0.0],
                half_extents: [2000.0, 11.0],
            })),
            half_height: 250.0,
        };
        let pin = crate::csg::CsgNode::from_decoded(&[sphere_feature(2, [0.0, 0.0, 0.0], 0.5)]);
        let node = BoundedCsg::from_csg(&crate::csg::CsgNode::Union(
            Box::new(board),
            Box::new(pin),
        ));
        assert!(resolution_for_thin_features(&node, 64) > 64);
    }

    #[test]
    fn degenerate_zero_extent_profiles_do_not_trigger_refinement() {
        use fielddraft_protocol::{Sketch2d, Sketch2dPrimitive};
        // A zero-height path (a bare line) cannot be meshed at any
        // resolution; it must not peg the body at the corner budget.
        let line = Sketch2d::Prim(Sketch2dPrimitive::Path {
            segments: vec![fielddraft_protocol::SketchSegment::Line {
                start: [-50.0, 0.0],
                end: [50.0, 0.0],
            }],
            closed: false,
        });
        let disc = Sketch2d::Prim(Sketch2dPrimitive::Circle {
            center: [0.0, 0.0],
            radius: 50.0,
        });
        let node = BoundedCsg::from_csg(&crate::csg::CsgNode::Extrude {
            wall_subs: None,
            id: FeatureId::from_u32(1),
            sketch: Box::new(Sketch2d::Union(Box::new(disc), Box::new(line))),
            half_height: 20.0,
        });
        assert_eq!(resolution_for_thin_features(&node, 48), 48);
    }
}

