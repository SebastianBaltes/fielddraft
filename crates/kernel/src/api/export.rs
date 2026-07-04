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
