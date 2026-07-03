//! Dokument-Lebenszyklus: Konstruktor, JSON-Load/-Save, FieldSkript-
//! Records, Rollback-Index und Undo/Redo.

use fielddraft_protocol::{Document};
use wasm_bindgen::prelude::*;

use crate::Kernel;
use crate::body;
use crate::{DEFAULT_CONTEXT_MESH_RESOLUTION, DEFAULT_MESH_RESOLUTION};
#[wasm_bindgen]
impl Kernel {
    /// Create a new kernel with an empty document.
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        #[cfg(target_arch = "wasm32")]
        console_error_panic_hook::set_once();

        let document = Document::default();
        let mut k = Self {
            document,
            decoded: Vec::new(),
            undo: Vec::new(),
            redo: Vec::new(),
            bodies: Vec::new(),
            active_mesh_resolution: DEFAULT_MESH_RESOLUTION,
            context_mesh_resolution: DEFAULT_CONTEXT_MESH_RESOLUTION,
            active_body_id: None,
            sync_meshing: true,
            mesh_algorithm: 0,
            parametric_weave: true,
            brep_suppressed: false,
            mesh_version: 0,
            edge_fillets: Vec::new(),
            mate_connectors: Vec::new(),
            feature_errors: std::collections::HashMap::new(),
            feature_warnings: std::collections::HashMap::new(),
            rollback_after: None,
            regen_brep_cache: body::BrepCache::new(),
        };
        // Seed the mesh buffer (empty document → empty mesh, but we
        // still want the version counter and buffers initialised).
        k.rebuild_mesh_internal();
        k
    }


    // ── document I/O ──────────────────────────────────────────────────

    #[wasm_bindgen(js_name = loadJson)]
    pub fn load_json(&mut self, json: &str) -> Result<usize, JsError> {
        let document: Document =
            serde_json::from_str(json).map_err(|e| JsError::new(&e.to_string()))?;
        self.document = document;

        // Self-heal documents written before the cascade-delete fix: a
        // delete used to leave gizmo transforms (translate/rotate/scale)
        // orphaned — their `target_id` points at a feature that no longer
        // exists — and such an orphan fails *every* regeneration. Prune
        // them on the way in so a stale `.fdraft` loads instead of dying
        // with "target feature N not found".
        self.prune_dangling_transform_calls();

        // DL-61 C2b: `feature_calls` is the sole wire form, so a load
        // always rebuilds the decoded spine by replaying the calls
        // through the feature registry. Any registry error surfaces
        // as a load failure (fail-stop per §4.6).
        self.bootstrap_features_from_calls()
            .map_err(|e| JsError::new(&format!("feature regeneration failed: {e}")))?;

        self.rebuild_packed();
        // Loading is a fresh start: history is irrelevant.
        self.undo.clear();
        self.redo.clear();
        Ok(self.decoded.len())
    }


    #[wasm_bindgen(js_name = toJson)]
    pub fn to_json(&self) -> Result<String, JsError> {
        serde_json::to_string(&self.document).map_err(|e| JsError::new(&e.to_string()))
    }

    #[wasm_bindgen(js_name = featureCount)]
    pub fn feature_count(&self) -> usize {
        self.decoded.len()
    }

    // ── FieldSkript records (S1) ──────────────────────────────────────
    //
    // Document-local, runtime-defined feature types. The kernel stores
    // them as opaque data (`Document.scripts`) so they round-trip via
    // `toJson`/`loadJson`; their `build()` bodies run web-side, so these
    // accessors never touch the decoded spine or the mesh. Mutations
    // snapshot for undo but skip `rebuild_packed` — no geometry changes.

    /// List the document's FieldSkript records as a JSON array.
    #[wasm_bindgen(js_name = listScripts)]
    pub fn list_scripts(&self) -> Result<String, JsError> {
        serde_json::to_string(&self.document.scripts)
            .map_err(|e| JsError::new(&e.to_string()))
    }

    /// Insert or replace a FieldSkript record (keyed by `id`). Takes
    /// the serialised `FieldSkriptRecord` JSON; returns `true` when it
    /// replaced an existing record, `false` when it inserted a new one.
    #[wasm_bindgen(js_name = upsertScript)]
    pub fn upsert_script(&mut self, json: &str) -> Result<bool, JsError> {
        let record: fielddraft_protocol::FieldSkriptRecord =
            serde_json::from_str(json).map_err(|e| JsError::new(&e.to_string()))?;
        self.snapshot();
        if let Some(slot) = self
            .document
            .scripts
            .iter_mut()
            .find(|r| r.id == record.id)
        {
            *slot = record;
            Ok(true)
        } else {
            self.document.scripts.push(record);
            Ok(false)
        }
    }

    /// Remove a FieldSkript record by id. Returns `true` when a record
    /// was removed.
    #[wasm_bindgen(js_name = removeScript)]
    pub fn remove_script(&mut self, id: &str) -> bool {
        // Snapshot only on an actual change so a no-op remove doesn't
        // pollute the undo stack.
        if !self.document.scripts.iter().any(|r| r.id == id) {
            return false;
        }
        self.snapshot();
        self.document.scripts.retain(|r| r.id != id);
        true
    }


    #[wasm_bindgen(js_name = setRollbackIndex)]
    pub fn set_rollback_index(&mut self, index: i32) {
        let n = self.document.feature_calls.len();
        let prev = self.rollback_after;
        self.rollback_after = if index < 0 || index as usize >= n {
            None
        } else {
            Some(index as usize)
        };
        if self.bootstrap_features_from_calls().is_err() {
            // Rolling to this position left the spine unbuildable.
            // Revert to the previous rollback position and rebuild so
            // we never render a torn, half-built spine.
            self.rollback_after = prev;
            let _ = self.bootstrap_features_from_calls();
        }
        self.rebuild_packed();
    }

    #[wasm_bindgen(js_name = getRollbackIndex)]
    pub fn get_rollback_index(&self) -> i32 {
        match self.rollback_after {
            Some(idx) => idx as i32,
            None => -1,
        }
    }


    // ── undo / redo ───────────────────────────────────────────────────

    #[wasm_bindgen(js_name = canUndo)]
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    #[wasm_bindgen(js_name = undoDepth)]
    pub fn undo_depth(&self) -> usize {
        self.undo.len()
    }

    #[wasm_bindgen(js_name = canRedo)]
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    #[wasm_bindgen(js_name = redoDepth)]
    pub fn redo_depth(&self) -> usize {
        self.redo.len()
    }

    pub fn undo(&mut self) -> bool {
        if let Some(prev) = self.undo.pop() {
            let current = std::mem::replace(&mut self.document, prev);
            self.redo.push(current);
            // DL-61 C2b: snapshots carry only `feature_calls`, so the
            // decoded spine must be regenerated from the restored
            // call list before re-meshing.
            let _ = self.bootstrap_features_from_calls();
            self.rebuild_packed();
            true
        } else {
            false
        }
    }

    pub fn redo(&mut self) -> bool {
        if let Some(next) = self.redo.pop() {
            let current = std::mem::replace(&mut self.document, next);
            self.undo.push(current);
            let _ = self.bootstrap_features_from_calls();
            self.rebuild_packed();
            true
        } else {
            false
        }
    }

}
