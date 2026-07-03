//! Dokument-Variablen: set/remove/rename/list.

use wasm_bindgen::prelude::*;

use crate::Kernel;
#[wasm_bindgen]
impl Kernel {
    /// K8.10: upsert a document-level variable. Snapshots so
    /// undo/redo walks variable edits the same way it walks
    /// feature edits.
    ///
    /// K8.11: after the write, re-bootstraps from calls so
    /// every formula that references the variable picks up
    /// the new value on the next mesh pass. Pre-K8.11 this
    /// was a pure state edit with no geometry side effect.
    #[wasm_bindgen(js_name = setVariable)]
    pub fn set_variable(
        &mut self,
        name: &str,
        value: f32,
        kind: &str,
        formula: Option<String>,
    ) -> Result<(), JsError> {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return Err(JsError::new("variable name must not be empty"));
        }
        if !value.is_finite() {
            return Err(JsError::new(&format!(
                "variable `{trimmed}` must be a finite number"
            )));
        }
        // An unknown / empty kind keeps the existing kind (when upserting) or
        // defaults to Number — so callers that don't care can pass "".
        let kind = match kind {
            "length" => fielddraft_protocol::VarKind::Length,
            "angle" => fielddraft_protocol::VarKind::Angle,
            "number" => fielddraft_protocol::VarKind::Number,
            _ => self
                .document
                .variables
                .get(trimmed)
                .map(|v| v.kind)
                .unwrap_or_default(),
        };
        // An empty formula string means "plain literal" (no source text to
        // remember) — collapse it to None so the wire form stays clean.
        let formula = formula.filter(|s| !s.trim().is_empty());
        self.snapshot();
        self.document.variables.insert(
            trimmed.to_string(),
            fielddraft_protocol::Variable::with_formula(value, kind, formula),
        );
        if let Err(err) = self.bootstrap_features_from_calls() {
            self.undo();
            return Err(JsError::new(&format!(
                "variable set failed to regenerate: {err}"
            )));
        }
        self.rebuild_packed();
        Ok(())
    }

    /// K8.10: remove a document-level variable. No-op if the
    /// name isn't in the table; returns `true` when it actually
    /// removed something.
    ///
    /// K8.11: re-bootstraps after the remove so any formula
    /// that referenced the departing variable surfaces as a
    /// regen error (which rolls the remove back).
    #[wasm_bindgen(js_name = removeVariable)]
    pub fn remove_variable(&mut self, name: &str) -> bool {
        if !self.document.variables.contains_key(name) {
            return false;
        }
        self.snapshot();
        self.document.variables.remove(name);
        if self.bootstrap_features_from_calls().is_err() {
            // A downstream formula broke — roll the remove
            // back. The caller gets `false` and the document
            // stays consistent.
            self.undo();
            return false;
        }
        self.rebuild_packed();
        true
    }

    /// Ticket 8ar9: rename a document-level variable, keeping its
    /// value. Errors on an empty new name or a collision with an
    /// existing variable; a no-op rename (`old == new`) succeeds.
    /// Re-bootstraps so any formula that referenced the old name
    /// surfaces as a regen error (which rolls the rename back) —
    /// future formula support will rewrite references here instead.
    #[wasm_bindgen(js_name = renameVariable)]
    pub fn rename_variable(&mut self, old: &str, new: &str) -> Result<(), JsError> {
        let new_name = new.trim();
        if new_name.is_empty() {
            return Err(JsError::new("variable name must not be empty"));
        }
        if old == new_name {
            return Ok(());
        }
        let Some(var) = self.document.variables.get(old).cloned() else {
            return Err(JsError::new(&format!("no variable named `{old}`")));
        };
        if self.document.variables.contains_key(new_name) {
            return Err(JsError::new(&format!(
                "a variable named `{new_name}` already exists"
            )));
        }
        self.snapshot();
        self.document.variables.remove(old);
        self.document
            .variables
            .insert(new_name.to_string(), var);
        if let Err(err) = self.bootstrap_features_from_calls() {
            self.undo();
            return Err(JsError::new(&format!(
                "variable rename failed to regenerate: {err}"
            )));
        }
        self.rebuild_packed();
        Ok(())
    }


    #[wasm_bindgen(js_name = listVariables)]
    pub fn list_variables(&self) -> Result<String, JsError> {
        #[derive(serde::Serialize)]
        struct Entry<'a> {
            name: &'a str,
            value: f32,
            kind: fielddraft_protocol::VarKind,
            #[serde(skip_serializing_if = "Option::is_none")]
            formula: Option<&'a str>,
        }
        let out: Vec<Entry> = self
            .document
            .variables
            .iter()
            .map(|(name, var)| Entry {
                name,
                value: var.value,
                kind: var.kind,
                formula: var.formula.as_deref(),
            })
            .collect();
        serde_json::to_string(&out).map_err(|e| JsError::new(&e.to_string()))
    }

}
