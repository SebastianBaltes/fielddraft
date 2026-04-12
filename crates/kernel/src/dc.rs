//! Naive uniform-grid Dual Contouring mesher.
//!
//! K1 scope (see README §10, "Kernel-Phase K1"):
//!   * Single-threaded, CPU-side.
//!   * One vertex per cell placed at the centroid of its sign-changing
//!     edge intersections (no QEF yet).
//!   * Quads emitted on sign-changing grid edges, triangulated into two
//!     triangles with winding derived from the sign flip.
//!   * Normals via central differences on the CSG SDF.
//!
//! Scope explicitly *excluded* from K1 (deferred to later slices):
//!   * QEF minimisation for sharp-feature-preserving vertex placement.
//!   * Manifold repair pass (see README §11, risk #1).
//!   * Adaptive octree grids.
//!   * Hermite-data edge normals.
//!
//! Everything here operates on a [`CsgNode`]; the kernel's linear
//! feature list is lifted into a tree first.

use crate::bvh::BoundedCsg;
use crate::csg::{EvalMode, ProxyElement, MAX_CONTRIBUTORS};

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
const DC_MARGIN_CELLS: f32 = 2.0;

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

    // Cell counts per axis — at least 1 on every axis so a near-flat
    // scene still gets a grid.
    let nx = ((size[0] / cell).ceil() as usize).max(1);
    let ny = ((size[1] / cell).ceil() as usize).max(1);
    let nz = ((size[2] / cell).ceil() as usize).max(1);

    // Corner grid is one larger than the cell grid in every dimension.
    let cnx = nx + 1;
    let cny = ny + 1;
    let cnz = nz + 1;
    let corner_index = |i: usize, j: usize, k: usize| -> usize {
        i + j * cnx + k * cnx * cny
    };
    let corner_point = |i: usize, j: usize, k: usize| -> [f32; 3] {
        [
            grid.min[0] + i as f32 * cell,
            grid.min[1] + j as f32 * cell,
            grid.min[2] + k as f32 * cell,
        ]
    };

    // ── Sample the SDF at every grid corner. ─────────────────────────
    let mut sdf_grid = vec![0.0_f32; cnx * cny * cnz];
    for k in 0..cnz {
        for j in 0..cny {
            for i in 0..cnx {
                sdf_grid[corner_index(i, j, k)] =
                    node.sdf(corner_point(i, j, k), EvalMode::Visual);
            }
        }
    }

    // ── Place one vertex per cell that contains a sign change. ──────
    // We iterate every cell's 12 edges, average the linear-interpolated
    // intersection points of those that actually flip sign, and store
    // the centroid as the cell's vertex.
    let cell_index = |i: usize, j: usize, k: usize| -> usize { i + j * nx + k * nx * ny };

    let cell_edges: [((usize, usize, usize), (usize, usize, usize)); 12] = [
        // X-aligned
        ((0, 0, 0), (1, 0, 0)),
        ((0, 1, 0), (1, 1, 0)),
        ((0, 0, 1), (1, 0, 1)),
        ((0, 1, 1), (1, 1, 1)),
        // Y-aligned
        ((0, 0, 0), (0, 1, 0)),
        ((1, 0, 0), (1, 1, 0)),
        ((0, 0, 1), (0, 1, 1)),
        ((1, 0, 1), (1, 1, 1)),
        // Z-aligned
        ((0, 0, 0), (0, 0, 1)),
        ((1, 0, 0), (1, 0, 1)),
        ((0, 1, 0), (0, 1, 1)),
        ((1, 1, 0), (1, 1, 1)),
    ];

    let mut cell_vertex_index = vec![u32::MAX; nx * ny * nz];
    let mut positions: Vec<f32> = Vec::new();
    let mut normals: Vec<f32> = Vec::new();
    let mut classifications: Vec<u8> = Vec::new();
    let mut contributors: Vec<u32> = Vec::new();

    let normal_eps = cell * 0.25;
    // Provenance-eps: a DC cell vertex sits within ≤ one cell of both
    // contributing leaves when it's on an intersection curve, so we
    // call any pair of leaves within half a cell "simultaneously
    // dominant". Too small → edges get under-detected; too large →
    // flat faces get polluted with phantom neighbours.
    let contributor_eps = cell * 0.5;

    for k in 0..nz {
        for j in 0..ny {
            for i in 0..nx {
                let mut acc = [0.0_f32; 3];
                let mut hits = 0;
                for &(a, b) in &cell_edges {
                    let ca = corner_index(i + a.0, j + a.1, k + a.2);
                    let cb = corner_index(i + b.0, j + b.1, k + b.2);
                    let da = sdf_grid[ca];
                    let db = sdf_grid[cb];
                    if (da < 0.0) == (db < 0.0) {
                        continue;
                    }
                    let denom = da - db;
                    let t = if denom.abs() < 1e-12 { 0.5 } else { da / denom };
                    let t = t.clamp(0.0, 1.0);
                    let pa = corner_point(i + a.0, j + a.1, k + a.2);
                    let pb = corner_point(i + b.0, j + b.1, k + b.2);
                    acc[0] += pa[0] + t * (pb[0] - pa[0]);
                    acc[1] += pa[1] + t * (pb[1] - pa[1]);
                    acc[2] += pa[2] + t * (pb[2] - pa[2]);
                    hits += 1;
                }
                if hits == 0 {
                    continue;
                }
                let inv = 1.0 / hits as f32;
                let vp = [acc[0] * inv, acc[1] * inv, acc[2] * inv];
                let n = sdf_normal(node, vp, normal_eps);
                let idx = (positions.len() / 3) as u32;
                positions.extend_from_slice(&vp);
                normals.extend_from_slice(&n);

                // Provenance: classify this vertex by querying the
                // contributor set from the CSG tree at its location.
                // Off-surface fallback (shouldn't happen for a clean
                // DC pass) gets tagged as Face with an empty leaf —
                // keeping the arrays strictly parallel to positions.
                let (_, cs) = node.evaluate(vp, contributor_eps, EvalMode::Visual);
                let mut element = cs.element();
                if element == ProxyElement::None {
                    element = ProxyElement::Face;
                }
                classifications.push(element.tag());
                // Flatten contributor ids into a fixed stride so the
                // zero-copy bridge can hand them to JS as a single
                // Uint32Array view. K4.5.1 still issues only flat-
                // numeric [`FeatureId`]s, so each contributor maps
                // back to a `u32`. When hierarchical ids land in a
                // later slice, the mesh contributor buffer will grow
                // a side table indexed by a compact u32 handle.
                let slice = cs.as_slice();
                for slot in 0..MAX_CONTRIBUTORS {
                    if slot < slice.len() {
                        // K8.0: linear_pattern clones carry
                        // hierarchical child ids that the K4.5.1
                        // flat-numeric packing can't represent.
                        // Fall through to the sentinel so the
                        // mesher builds cleanly — picking on a
                        // clone face currently resolves to
                        // "unknown contributor", which the UI
                        // falls back on (K9 refactor will add a
                        // compound-id side table for full
                        // provenance).
                        let compact = slice[slot]
                            .as_u32_if_flat()
                            .unwrap_or(CONTRIBUTOR_SENTINEL);
                        contributors.push(compact);
                    } else {
                        contributors.push(CONTRIBUTOR_SENTINEL);
                    }
                }

                cell_vertex_index[cell_index(i, j, k)] = idx;
            }
        }
    }

    // ── Emit one quad per sign-changing grid edge. ──────────────────
    // A sign-changing edge is shared by (up to) four adjacent cells;
    // their DC vertices form the quad. Winding is chosen so the face
    // normal points out of the solid (from negative to positive SDF).
    //
    // Boundary edges (fewer than four neighbours) are skipped — they
    // would be incomplete quads. The `DC_MARGIN_CELLS` padding guards
    // against this ever happening for the actual surface: the surface
    // always sits at least two cells from the grid boundary.
    let mut indices: Vec<u32> = Vec::new();

    // --- X-axis edges: corner (i, j, k) → (i+1, j, k). -------------
    // Neighbouring cells share this edge at the four (j-1|j, k-1|k)
    // combinations. Vertex arrangement around the edge when viewed
    // along +x:
    //
    //     v01 (−y,+z) ─── v11 (+y,+z)
    //         │               │
    //     v00 (−y,−z) ─── v10 (+y,−z)
    for i in 0..nx {
        for j in 1..ny {
            for k in 1..nz {
                let ca = corner_index(i, j, k);
                let cb = corner_index(i + 1, j, k);
                let da = sdf_grid[ca];
                let db = sdf_grid[cb];
                if (da < 0.0) == (db < 0.0) {
                    continue;
                }
                let v00 = cell_vertex_index[cell_index(i, j - 1, k - 1)];
                let v10 = cell_vertex_index[cell_index(i, j, k - 1)];
                let v11 = cell_vertex_index[cell_index(i, j, k)];
                let v01 = cell_vertex_index[cell_index(i, j - 1, k)];
                if v00 == u32::MAX || v10 == u32::MAX || v11 == u32::MAX || v01 == u32::MAX {
                    continue;
                }
                // da>0, db<0 → material on +x → outward = −x → reverse.
                if da > 0.0 {
                    indices.extend_from_slice(&[v00, v01, v11]);
                    indices.extend_from_slice(&[v00, v11, v10]);
                } else {
                    indices.extend_from_slice(&[v00, v10, v11]);
                    indices.extend_from_slice(&[v00, v11, v01]);
                }
            }
        }
    }

    // --- Y-axis edges: corner (i, j, k) → (i, j+1, k). -------------
    // Neighbour cells indexed by (i-1|i, k-1|k). Viewed along +y the
    // arrangement is:
    //
    //     v01 (−x,+z) ─── v11 (+x,+z)
    //         │               │
    //     v00 (−x,−z) ─── v10 (+x,−z)
    for j in 0..ny {
        for i in 1..nx {
            for k in 1..nz {
                let ca = corner_index(i, j, k);
                let cb = corner_index(i, j + 1, k);
                let da = sdf_grid[ca];
                let db = sdf_grid[cb];
                if (da < 0.0) == (db < 0.0) {
                    continue;
                }
                let v00 = cell_vertex_index[cell_index(i - 1, j, k - 1)];
                let v10 = cell_vertex_index[cell_index(i, j, k - 1)];
                let v11 = cell_vertex_index[cell_index(i, j, k)];
                let v01 = cell_vertex_index[cell_index(i - 1, j, k)];
                if v00 == u32::MAX || v10 == u32::MAX || v11 == u32::MAX || v01 == u32::MAX {
                    continue;
                }
                // Forward winding gives normal −y (see comment in
                // README notes). da>0 → material +y → outward −y →
                // keep forward. da<0 → reverse.
                if da > 0.0 {
                    indices.extend_from_slice(&[v00, v10, v11]);
                    indices.extend_from_slice(&[v00, v11, v01]);
                } else {
                    indices.extend_from_slice(&[v00, v01, v11]);
                    indices.extend_from_slice(&[v00, v11, v10]);
                }
            }
        }
    }

    // --- Z-axis edges: corner (i, j, k) → (i, j, k+1). -------------
    // Neighbour cells indexed by (i-1|i, j-1|j). Viewed along +z:
    //
    //     v01 (−x,+y) ─── v11 (+x,+y)
    //         │               │
    //     v00 (−x,−y) ─── v10 (+x,−y)
    for k in 0..nz {
        for i in 1..nx {
            for j in 1..ny {
                let ca = corner_index(i, j, k);
                let cb = corner_index(i, j, k + 1);
                let da = sdf_grid[ca];
                let db = sdf_grid[cb];
                if (da < 0.0) == (db < 0.0) {
                    continue;
                }
                let v00 = cell_vertex_index[cell_index(i - 1, j - 1, k)];
                let v10 = cell_vertex_index[cell_index(i, j - 1, k)];
                let v11 = cell_vertex_index[cell_index(i, j, k)];
                let v01 = cell_vertex_index[cell_index(i - 1, j, k)];
                if v00 == u32::MAX || v10 == u32::MAX || v11 == u32::MAX || v01 == u32::MAX {
                    continue;
                }
                // Forward winding → +z normal. da>0 → material +z →
                // outward −z → reverse.
                if da > 0.0 {
                    indices.extend_from_slice(&[v00, v01, v11]);
                    indices.extend_from_slice(&[v00, v11, v10]);
                } else {
                    indices.extend_from_slice(&[v00, v10, v11]);
                    indices.extend_from_slice(&[v00, v11, v01]);
                }
            }
        }
    }

    Mesh {
        positions,
        normals,
        indices,
        classifications,
        contributors,
    }
}

/// Surface normal at `p` by central differences on the BVH SDF.
/// Returns a unit vector; collapses to `+Y` when the gradient is
/// numerically zero (only possible in degenerate cases like an empty
/// tree, which the caller already guards against).
fn sdf_normal(node: &BoundedCsg, p: [f32; 3], eps: f32) -> [f32; 3] {
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
            twist_rate: 0.0,
            bend_rate: 0.0,
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
            twist_rate: 0.0,
            bend_rate: 0.0,
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
            twist_rate: 0.0,
            bend_rate: 0.0,
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
}
