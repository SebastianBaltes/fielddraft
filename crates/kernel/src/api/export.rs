//! Export-API: STL (ASCII/binär), 3MF, STEP-Klassifikation und
//! STEP-Export.

use wasm_bindgen::prelude::*;

use crate::Kernel;
use crate::export;
use crate::export_3mf;
use crate::step_classifier;
use crate::step_commands;
#[wasm_bindgen]
impl Kernel {
    // ── Meshing Phase 0: tolerance-driven export meshing ─────────
    //
    // The mesh export no longer ships the viewport mesh: a session
    // builds every body at its own cell-size-derived resolution and
    // validates the result (watertight + measured SDF deviation).
    // Protocol: `exportMeshBegin(cellMm)` → N× `exportMeshBody(i)`
    // (the caller loops and reports progress) → `exportMeshStl()` /
    // `exportMesh3mf()` + `exportMeshReport()` → `exportMeshEnd()`.
    // Any document mutation between calls drops the session.

    /// Start an export meshing pass at `cell_size_mm`. Returns the
    /// number of bodies the caller must feed through
    /// [`Kernel::export_mesh_body`].
    #[wasm_bindgen(js_name = exportMeshBegin)]
    pub fn export_mesh_begin(&mut self, cell_size_mm: f32, method: String) -> usize {
        let method = crate::export_mesh::MeshMethod::from_wire(&method);
        self.export_mesh_begin_internal(cell_size_mm, method)
    }

    /// Debug: sample the scene SDF on a `resolution`×`resolution` grid over a
    /// plane centred at `(ox,oy,oz)` with `(nx,ny,nz)` normal, spanning
    /// ±`half_extent` in each in-plane axis. Row-major (v outer, u inner). The
    /// scene SDF is the min over all bodies of each body's exact mesher SDF
    /// (`EvalMode::Visual`) — the same field the meshers see — so the SDF-plane
    /// overlay shows exactly what is being contoured. Empty scene → all `MAX`.
    #[wasm_bindgen(js_name = sdfPlaneSample)]
    #[allow(clippy::too_many_arguments)]
    pub fn sdf_plane_sample(
        &self,
        ox: f32,
        oy: f32,
        oz: f32,
        nx: f32,
        ny: f32,
        nz: f32,
        half_extent: f32,
        resolution: u32,
    ) -> Vec<f32> {
        let res = (resolution.max(2)) as usize;
        // Orthonormal in-plane frame (u, v) from the normal.
        let mut n = [nx, ny, nz];
        let nl = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        if nl < 1e-9 {
            n = [0.0, 0.0, 1.0];
        } else {
            n = [n[0] / nl, n[1] / nl, n[2] / nl];
        }
        let up = if n[1].abs() < 0.9 { [0.0, 1.0, 0.0] } else { [1.0, 0.0, 0.0] };
        let cross = |a: [f32; 3], b: [f32; 3]| {
            [
                a[1] * b[2] - a[2] * b[1],
                a[2] * b[0] - a[0] * b[2],
                a[0] * b[1] - a[1] * b[0],
            ]
        };
        let norm = |a: [f32; 3]| {
            let l = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt().max(1e-9);
            [a[0] / l, a[1] / l, a[2] / l]
        };
        let u = norm(cross(up, n));
        let v = cross(n, u);

        // Build each body's bounded CSG once; the scene field is their union.
        let bodies: Vec<crate::bvh::BoundedCsg> = self
            .bodies
            .iter()
            .map(|b| {
                crate::body::build_bounded_with_primitives(&self.decoded, &b.feature_indices).0
            })
            .collect();

        let step = if res > 1 { (2.0 * half_extent) / (res as f32 - 1.0) } else { 0.0 };
        let mut out = Vec::with_capacity(res * res);
        for j in 0..res {
            let tv = -half_extent + j as f32 * step;
            for i in 0..res {
                let tu = -half_extent + i as f32 * step;
                let p = [
                    ox + u[0] * tu + v[0] * tv,
                    oy + u[1] * tu + v[1] * tv,
                    oz + u[2] * tu + v[2] * tv,
                ];
                let mut d = f32::MAX;
                for b in &bodies {
                    d = d.min(b.sdf(p, crate::csg::EvalMode::Visual));
                }
                out.push(d);
            }
        }
        out
    }

    /// Debug ▸ Mesher: build the narrow-band adaptive octree over the scene
    /// (one point per leaf cell = its centre) and run the OPTIONAL stages of
    /// the debug meshing pipeline, all controlled by `options_json`
    /// ([`crate::mesher_points::MesherPointOptions`], camelCase; unknown
    /// fields ignored, malformed JSON → defaults):
    ///
    ///  1. jq2t point optimisers (processing filters, Newton "Fit on
    ///     Surface", tangential "Repulsion inside Surface", cell clamping),
    ///  2. `meshPoints`: adaptive dual contouring over the optimised points
    ///     (one vertex per leaf; quads at sign-changing minimal octree
    ///     edges), after an optional 2:1 balance pass (`balanceOctree`).
    ///
    /// Returns `[nPoints, nTris, points…, indices…]`: per leaf 9 floats
    /// `[x, y, z, sdf, nx, ny, nz, cellSize, shown]` with the FINAL point
    /// position (sdf/normal re-evaluated there) and a `shown` display flag
    /// (1 = passed the near-surface/sign-change filters), then 3 indices per
    /// triangle into the point list. Points failing the processing filters
    /// stay rigid and `shown = 0`; "only on surface" escapees are dropped
    /// from the point list entirely.
    #[wasm_bindgen(js_name = mesherPoints)]
    pub fn mesher_points(&mut self, options_json: String) -> Vec<f32> {
        let opts: crate::mesher_points::MesherPointOptions =
            serde_json::from_str(&options_json).unwrap_or_default();
        let bodies: Vec<crate::bvh::BoundedCsg> = self
            .bodies
            .iter()
            .map(|b| {
                crate::body::build_bounded_with_primitives(&self.decoded, &b.feature_indices).0
            })
            .collect();
        let Some((center, root_size)) = octree_root(&bodies) else {
            return Vec::new();
        };
        let scene_sdf = |p: [f32; 3]| {
            let mut d = f32::MAX;
            for b in &bodies {
                d = d.min(b.sdf(p, crate::csg::EvalMode::Visual));
            }
            d
        };
        let mut root = crate::mesher_points::build_octree(
            center,
            root_size,
            opts.max_depth,
            opts.band_guard,
            &scene_sdf,
        );
        if opts.mesh_points && opts.balance_octree {
            crate::mesher_points::balance_2to1(&mut root, &scene_sdf);
        }
        let min_size_est = root_size / (1u32 << opts.max_depth.min(10)) as f32;
        let eps = (min_size_est * 0.05).clamp(1e-3, 1.0);
        let grad = |p: [f32; 3]| -> [f32; 3] {
            let gx = scene_sdf([p[0] + eps, p[1], p[2]]) - scene_sdf([p[0] - eps, p[1], p[2]]);
            let gy = scene_sdf([p[0], p[1] + eps, p[2]]) - scene_sdf([p[0], p[1] - eps, p[2]]);
            let gz = scene_sdf([p[0], p[1], p[2] + eps]) - scene_sdf([p[0], p[1], p[2] - eps]);
            let l = (gx * gx + gy * gy + gz * gz).sqrt();
            if l > 1e-12 {
                [gx / l, gy / l, gz / l]
            } else {
                [0.0, 0.0, 1.0]
            }
        };
        let leaves = crate::mesher_points::collect_leaves(&mut root, &grad);
        if leaves.is_empty() {
            return vec![0.0, 0.0];
        }

        // ── B-Rep skeleton (Hybrid-PASM P1, ticket nwmi) ──
        // The woven edges become FIXED particle chains: corners exactly at the
        // B-Rep vertices (flag 3), edge interiors resampled by arc length at
        // the interaction radius (flag 2). They repel the free particles but
        // never move — sharp features are boundary conditions, not detection
        // targets. Positions fold the body placement (mirror body_brep_edges).
        let mut skeleton: Vec<[f32; 3]> = Vec::new();
        let mut skeleton_corner: Vec<bool> = Vec::new();
        if opts.brep_skeleton {
            let h = if opts.interaction_radius > 0.0 {
                opts.interaction_radius
            } else {
                min_size_est
            };
            for bi in 0..self.bodies.len() {
                self.bodies[bi].ensure_brep(&self.decoded, true);
                let placement = self.bodies[bi].placement;
                let Some(brep) = self.bodies[bi].brep_cached() else { continue };
                let s = placement.scale[0];
                let to_world = |p: [f64; 3]| -> [f32; 3] {
                    let scaled = [p[0] as f32 * s, p[1] as f32 * s, p[2] as f32 * s];
                    let r = crate::quat_rotate(placement.rotation, scaled);
                    [
                        r[0] + placement.translate[0],
                        r[1] + placement.translate[1],
                        r[2] + placement.translate[2],
                    ]
                };
                for v in &brep.vertices {
                    skeleton.push(to_world(v.position));
                    skeleton_corner.push(true);
                }
                for e in &brep.edges {
                    let pts: Vec<[f32; 3]> = crate::brep::edge_sampling::sample_edge(&e.geometry)
                        .into_iter()
                        .map(to_world)
                        .collect();
                    let chain = crate::mesher_points::resample_polyline(&pts, h);
                    // Interior points only: chain endpoints coincide with the
                    // corner vertices already emitted above (open edges) or
                    // wrap onto the chain itself (closed rings — keep the seam
                    // point once as a plain edge sample).
                    let start = if e.start.is_some() { 1 } else { 0 };
                    let end = chain.len().saturating_sub(usize::from(e.end.is_some()));
                    for p in &chain[start.min(chain.len())..end] {
                        skeleton.push(*p);
                        skeleton_corner.push(false);
                    }
                }
            }
        }

        let (points, processed, alive) =
            crate::mesher_points::optimize_leaf_points(&leaves, &opts, &scene_sdf, &grad, &skeleton);
        // The mesh connects exactly the SHOWN points (passed the display
        // filters = processed AND survived only-on-surface): one vertex per
        // shown leaf, triangles by octree-edge adjacency (no sign-change).
        let shown: Vec<bool> = (0..leaves.len()).map(|i| processed[i] && alive[i]).collect();
        let indices = if opts.mesh_points {
            crate::mesher_points::connect_octree_edges(&root, &shown)
        } else {
            Vec::new()
        };

        // "Only on surface" drops points from the SET: dead leaves are
        // omitted from the output and the mesh indices remapped onto the
        // surviving points.
        let mut remap = vec![u32::MAX; leaves.len()];
        let mut n_alive = 0u32;
        for (i, &a) in alive.iter().enumerate() {
            if a {
                remap[i] = n_alive;
                n_alive += 1;
            }
        }
        // Per point 9 floats: [x, y, z, sdf, nx, ny, nz, size, shown]. `shown`
        // (1/0) = passed the DISPLAY filters (near-surface AND sign-change) —
        // the overlay draws normals/dots only for shown points, so toggling a
        // processing filter visibly culls the cloud (tickets jt76/gjuu). The
        // MESH still uses every emitted point as a vertex (crack-free DC), so
        // `shown` is display-only; only "only-on-surface" escapees are gone
        // from the vertex set entirely (dropped above via `alive`).
        let mut out =
            Vec::with_capacity(2 + (n_alive as usize + skeleton.len()) * 9 + indices.len());
        out.push((n_alive as usize + skeleton.len()) as f32);
        out.push((indices.len() / 3) as f32);
        for (i, leaf) in leaves.iter().enumerate() {
            if !alive[i] {
                continue;
            }
            let p = points[i];
            let (d, n) = if processed[i] && (opts.fit_on_surface || opts.repulsion) {
                (scene_sdf(p), grad(p))
            } else {
                (leaf.sdf, leaf.normal)
            };
            let shown = if processed[i] { 1.0 } else { 0.0 };
            out.extend_from_slice(&[p[0], p[1], p[2], d, n[0], n[1], n[2], leaf.size, shown]);
        }
        // Skeleton points: flag 2 = edge chain, 3 = corner (drawn distinctly
        // by the overlay; the octree mesh never references them, so appending
        // after the leaf points keeps every triangle index valid).
        for (k, p) in skeleton.iter().enumerate() {
            let flag = if skeleton_corner[k] { 3.0 } else { 2.0 };
            out.extend_from_slice(&[p[0], p[1], p[2], 0.0, 0.0, 0.0, 1.0, 0.0, flag]);
        }
        out.extend(indices.iter().map(|&i| remap[i as usize] as f32));
        out
    }

    /// World-space centre of the *visible* solids' AABB — the orbit pivot
    /// (ticket f88u). This is the union of the per-body `Body.aabb`, which the
    /// feature pipeline keeps analytically current: a boolean Remove retreats
    /// the bound past the slab it caps, an Intersect clips it to the tool, so a
    /// half-carved sphere reports the visible remainder — unlike
    /// [`Kernel::scene_aabb`], whose Difference bound stays the conservative
    /// *left operand* (the phantom whole sphere) for clip-plane framing. `[]`
    /// for an empty scene → the caller keeps the conservative centre. Cheap
    /// (unions a handful of stored boxes), so it needs no memoisation.
    #[wasm_bindgen(js_name = sceneVisibleCenter)]
    pub fn scene_visible_center(&self) -> Vec<f32> {
        let mut bb = crate::csg::Aabb::EMPTY;
        for b in &self.bodies {
            bb = crate::csg::Aabb::union(bb, b.aabb);
        }
        if bb.is_empty() {
            return Vec::new();
        }
        vec![
            0.5 * (bb.min[0] + bb.max[0]),
            0.5 * (bb.min[1] + bb.max[1]),
            0.5 * (bb.min[2] + bb.max[2]),
        ]
    }

    /// Pre-mesh estimate for the export dialog: for dense-grid methods, the
    /// finest achievable cell size (memory-guard lower bound) and a rough
    /// raw-grid triangle count at `cell_size_mm`, as JSON
    /// ([`crate::export_mesh::ExportEstimate`]). Read-only and cheap, so the
    /// dialog can call it live while the user edits the cell size / method.
    #[wasm_bindgen(js_name = exportEstimate)]
    pub fn export_estimate(&self, cell_size_mm: f32, method: String) -> String {
        let method = crate::export_mesh::MeshMethod::from_wire(&method);
        serde_json::to_string(&self.export_estimate_internal(cell_size_mm, method))
            .unwrap_or_default()
    }

    /// Mesh + validate body `i` for the running session. Returns the
    /// per-body report as JSON (`""` when no session is active).
    #[wasm_bindgen(js_name = exportMeshBody)]
    pub fn export_mesh_body(&mut self, i: usize) -> String {
        match self.export_mesh_body_internal(i) {
            Some(report) => serde_json::to_string(&report).unwrap_or_default(),
            None => String::new(),
        }
    }

    /// Binary STL of the session's export meshes.
    #[wasm_bindgen(js_name = exportMeshStl)]
    pub fn export_mesh_stl(&self) -> Vec<u8> {
        export::export_binary(&self.export_session_meshes())
    }

    /// ASCII STL of the session's export meshes (debug / diffing).
    #[wasm_bindgen(js_name = exportMeshStlAscii)]
    pub fn export_mesh_stl_ascii(&self) -> String {
        export::export_ascii(&self.export_session_meshes())
    }

    /// 3MF archive of the session's export meshes.
    #[wasm_bindgen(js_name = exportMesh3mf)]
    pub fn export_mesh_3mf(&self) -> Vec<u8> {
        export_3mf::export_3mf(&self.export_session_meshes())
    }

    /// Merged vertex positions (xyz-interleaved) of ALL session export
    /// meshes, for the in-app solid preview. Pairs with
    /// [`Kernel::export_mesh_preview_indices`] (indices are rebased per
    /// body). Flat shading derives face normals in the viewer, so no
    /// normal buffer crosses the boundary.
    #[wasm_bindgen(js_name = exportMeshPreviewPositions)]
    pub fn export_mesh_preview_positions(&self) -> Vec<f32> {
        let mut out = Vec::new();
        for m in self.export_session_meshes() {
            out.extend_from_slice(&m.positions);
        }
        out
    }

    /// Merged triangle indices of ALL session export meshes, rebased so
    /// they index into the concatenated
    /// [`Kernel::export_mesh_preview_positions`] buffer.
    #[wasm_bindgen(js_name = exportMeshPreviewIndices)]
    pub fn export_mesh_preview_indices(&self) -> Vec<u32> {
        let mut out = Vec::new();
        let mut base = 0u32;
        for m in self.export_session_meshes() {
            out.extend(m.indices.iter().map(|&idx| idx + base));
            base += (m.positions.len() / 3) as u32;
        }
        out
    }

    /// Aggregate report of the session as JSON
    /// (see [`crate::export_mesh::ExportReport`]).
    #[wasm_bindgen(js_name = exportMeshReport)]
    pub fn export_mesh_report(&self) -> String {
        match &self.export_session {
            Some(s) => serde_json::to_string(&s.report()).unwrap_or_default(),
            None => String::new(),
        }
    }

    /// Drop the session and free its meshes.
    #[wasm_bindgen(js_name = exportMeshEnd)]
    pub fn export_mesh_end(&mut self) {
        self.export_session = None;
    }

    // ── M14b: STEP classification ─────────────────────────────────

    #[wasm_bindgen(js_name = classifyForStep)]
    pub fn classify_for_step(&self) -> String {
        let report = step_classifier::classify(&self.decoded);
        report.to_json().to_string()
    }

    #[wasm_bindgen(js_name = stepCommands)]
    pub fn step_commands(&self) -> String {
        let cmds = step_commands::build_step_commands(&self.decoded);
        cmds.to_string()
    }

    /// Serialise body `i`'s feature-based B-Rep as an ISO-10303-21
    /// (STEP) AP214 advanced-B-Rep file. Reuses the same lazily-woven,
    /// cached B-Rep as [`Kernel::body_brep_counts`]; the returned
    /// string is a complete `.step` document ready to download. The
    /// product name is derived from the body's stable feature id.
    #[wasm_bindgen(js_name = bodyStepExport)]
    pub fn body_step_export(&mut self, i: usize) -> String {
        let product_name = format!("body_{}", self.bodies[i].id);
        self.bodies[i].ensure_brep(&self.decoded, self.parametric_weave);
        let placement = self.bodies[i].placement;
        let brep = self.bodies[i].brep_cached().expect("woven above");
        crate::brep::step_export::export_step_placed(brep, &placement, &product_name)
    }

    /// Serialise *every* body's feature-based B-Rep into ONE ISO-10303-21
    /// (STEP) AP214 file, each body a separate `MANIFOLD_SOLID_BREP` solid
    /// in a single shape representation (DL-53: bodies stay separate). Each
    /// body's B-Rep is lazily woven and cached exactly as in
    /// [`Kernel::body_step_export`]; the returned string is a complete
    /// `.step` document ready to download.
    #[wasm_bindgen(js_name = sceneStepExport)]
    pub fn scene_step_export(&mut self) -> String {
        // Weave every body first (mutates each body's cache), then borrow the
        // breps immutably for export — two passes so the mutable `ensure_brep`
        // borrows don't overlap the shared-slice borrow.
        for i in 0..self.bodies.len() {
            self.bodies[i].ensure_brep(&self.decoded, self.parametric_weave);
        }
        let breps: Vec<(&crate::brep::BRep, fielddraft_protocol::Transform)> = self
            .bodies
            .iter()
            .map(|b| (b.brep_cached().expect("woven above"), b.placement))
            .collect();
        crate::brep::step_export::export_step_scene_placed(&breps, "scene")
    }

}

/// Debug ▸ Mesher core (kept free of the wasm boundary so it is unit-testable):
/// build a narrow-band adaptive octree over the union of `bodies` and return
/// every leaf cell centre packed `[x, y, z, sdf, nx, ny, nz, cellSize]`.
#[cfg(test)]
pub(crate) fn octree_leaf_samples(
    bodies: &[crate::bvh::BoundedCsg],
    max_depth: u32,
    band_guard: f32,
) -> Vec<f32> {
    octree_leaves(bodies, max_depth, band_guard)
        .into_iter()
        .flat_map(|l| {
            [
                l.center[0], l.center[1], l.center[2], l.sdf, l.normal[0], l.normal[1],
                l.normal[2], l.size,
            ]
        })
        .collect()
}

/// The leaf structs behind [`octree_leaf_samples`] — the jq2t point pipeline
/// needs the cells (centre + size = clamp bounds), not just packed floats.
/// Root sizing lives here; construction is `mesher_points::build_octree`.
pub(crate) fn octree_root(bodies: &[crate::bvh::BoundedCsg]) -> Option<([f32; 3], f32)> {
    if bodies.is_empty() {
        return None;
    }
    let mut mn = [f32::MAX; 3];
    let mut mx = [f32::MIN; 3];
    let mut any = false;
    for b in bodies {
        let a = b.aabb();
        if !a.is_empty() {
            any = true;
            for k in 0..3 {
                mn[k] = mn[k].min(a.min[k]);
                mx[k] = mx[k].max(a.max[k]);
            }
        }
    }
    if !any {
        mn = [-50.0; 3];
        mx = [50.0; 3];
    }
    let center = [
        0.5 * (mn[0] + mx[0]),
        0.5 * (mn[1] + mx[1]),
        0.5 * (mn[2] + mx[2]),
    ];
    let ext = (mx[0] - mn[0]).max(mx[1] - mn[1]).max(mx[2] - mn[2]);
    Some((center, (ext * 1.1).max(1e-3)))
}

pub(crate) fn octree_leaves(
    bodies: &[crate::bvh::BoundedCsg],
    max_depth: u32,
    band_guard: f32,
) -> Vec<crate::mesher_points::Leaf> {
    let Some((center, root_size)) = octree_root(bodies) else {
        return Vec::new();
    };
    let scene_sdf = |p: [f32; 3]| {
        let mut d = f32::MAX;
        for b in bodies {
            d = d.min(b.sdf(p, crate::csg::EvalMode::Visual));
        }
        d
    };
    let eps = (root_size * 0.005).clamp(1e-3, 1.0);
    let grad = |p: [f32; 3]| -> [f32; 3] {
        let gx = scene_sdf([p[0] + eps, p[1], p[2]]) - scene_sdf([p[0] - eps, p[1], p[2]]);
        let gy = scene_sdf([p[0], p[1] + eps, p[2]]) - scene_sdf([p[0], p[1] - eps, p[2]]);
        let gz = scene_sdf([p[0], p[1], p[2] + eps]) - scene_sdf([p[0], p[1], p[2] - eps]);
        let gl = (gx * gx + gy * gy + gz * gz).sqrt();
        if gl > 1e-12 {
            [gx / gl, gy / gl, gz / gl]
        } else {
            [0.0, 0.0, 1.0]
        }
    };
    let mut root =
        crate::mesher_points::build_octree(center, root_size, max_depth, band_guard, &scene_sdf);
    crate::mesher_points::collect_leaves(&mut root, &grad)
}

#[cfg(test)]
mod mesher_octree_tests {
    use super::octree_leaf_samples;
    use crate::bvh::BoundedCsg;
    use crate::csg::{CsgNode, DecodedFeature, PrimitiveModifiers};
    use fielddraft_protocol::{BoolOp, FeatureId, Primitive, Transform};

    fn sphere(radius: f32) -> BoundedCsg {
        let f = DecodedFeature::from_parts(
            FeatureId::from_u32(1),
            "s".into(),
            BoolOp::New,
            Primitive::Sphere {
                center: [0.0, 0.0, 0.0],
                radius,
            },
            Transform::IDENTITY,
            PrimitiveModifiers::NONE,
            vec![],
        );
        BoundedCsg::from_csg(&CsgNode::from_decoded(&[f]))
    }

    #[test]
    fn octree_is_adaptive_with_outward_normals_on_a_sphere() {
        let bodies = [sphere(10.0)];
        let out = octree_leaf_samples(&bodies, 5, 1.0);
        assert!(!out.is_empty() && out.len() % 8 == 0);
        let n = out.len() / 8;

        // Adaptive: cells near the surface are the FINEST, cells far away stay
        // coarse — so more than one distinct cell size appears.
        let mut sizes: Vec<f32> = (0..n).map(|k| out[k * 8 + 7]).collect();
        sizes.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert!(
            *sizes.last().unwrap() > *sizes.first().unwrap() * 1.5,
            "expected a range of cell sizes (adaptive), got {sizes:?}"
        );

        // The finest cells cluster on the surface; the gradient there points
        // radially outward (n · p̂ ≈ 1 for a sphere).
        let mut checked = 0;
        for k in 0..n {
            let b = k * 8;
            let (x, y, z, d) = (out[b], out[b + 1], out[b + 2], out[b + 3]);
            let (nx, ny, nz) = (out[b + 4], out[b + 5], out[b + 6]);
            let r = (x * x + y * y + z * z).sqrt();
            if d.abs() < 1.0 && r > 1e-3 {
                let dot = (nx * x + ny * y + nz * z) / r;
                assert!(dot > 0.9, "surface normal should point outward, dot={dot}");
                checked += 1;
            }
        }
        assert!(checked > 0, "expected some near-surface leaf cells");
    }

    #[test]
    fn empty_scene_yields_no_cells() {
        assert!(octree_leaf_samples(&[], 5, 1.0).is_empty());
    }

    /// Hybrid-PASM P1 (ticket nwmi): with `brepSkeleton` on, a box body emits
    /// its 8 corners as pinned flag-3 particles EXACTLY at the corner
    /// positions plus arc-length-resampled flag-2 chains along the 12 edges.
    #[test]
    fn mesher_points_emits_brep_skeleton_for_a_box() {
        use fielddraft_protocol::{BoolOp, Primitive};
        let mut k = crate::Kernel::new();
        k.add_primitive(
            Primitive::KIND_BOX,
            BoolOp::TAG_NEW,
            0.0,
            0.0,
            0.0,
            10.0,
            10.0,
            10.0,
        )
        .unwrap();
        let out = k.mesher_points(
            r#"{"maxDepth":3,"brepSkeleton":true,"interactionRadius":2.0}"#.into(),
        );
        let total = out[0] as usize;
        let mut corners = Vec::new();
        let mut chain = 0usize;
        for i in 0..total {
            let b = 2 + i * 9;
            match out[b + 8] as i32 {
                3 => corners.push([out[b], out[b + 1], out[b + 2]]),
                2 => chain += 1,
                _ => {}
            }
        }
        assert_eq!(corners.len(), 8, "eight pinned box corners");
        for c in &corners {
            for a in 0..3 {
                assert!(
                    (c[a].abs() - 10.0).abs() < 1e-3,
                    "corner coordinate off ±10: {c:?}"
                );
            }
        }
        // 12 edges of length 20 at h=2 → 9 interior points each = 108.
        assert_eq!(chain, 12 * 9, "arc-length chains at the interaction radius");
    }
}
