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
    pub fn export_mesh_begin(&mut self, cell_size_mm: f32) -> usize {
        self.export_mesh_begin_internal(cell_size_mm)
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
