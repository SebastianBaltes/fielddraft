//! FieldSkript S2a — the re-entrancy-safe live-context scratch.
//!
//! Foundation for the Rust→JS script callback (§2 of
//! `docs/FIELDSKRIPT-CONCEPT.md`). When `regenerate` reaches a script
//! feature it hands control to a JS `build()` running in the worker
//! realm; that `build()` calls back **synchronously** into the kernel
//! to commit built-in ops into the geometry under construction
//! (`fd.op*`). Because the outer `regenerate` is already on the wasm
//! stack holding `&mut Kernel`, a re-entrant commit can't take
//! `&mut self` again — wasm-bindgen panics on recursive use of the same
//! exported object. So the in-flight accumulator lives in a
//! `thread_local` `RefCell` **outside** any kernel borrow, and the
//! commit is a *free function* that reaches it with a short, disjoint
//! borrow. That is the single-writer borrow discipline of §2.6:
//! the loop holds no borrow across the callback, each re-entry borrows
//! briefly and gives it straight back.
//!
//! **The wasm boundary carries op-descriptors, never serialized IR.**
//! `CsgNode` is not `Serialize`, and it doesn't need to be: a script
//! never sees a geometry node. `commit_builtin` takes a `type_name` +
//! params blob and **reuses the built-in's existing `execute`** to
//! produce the geometry, then keeps the resulting `DecodedFeature`s in
//! the scratch. So the pure eval and the commit are one Rust call, the
//! geometry logic stays a single path (concept #2 — no duplicating
//! built-ins), and the IR never crosses into JS.
//!
//! ## What this module is (S2a) and is not (yet)
//!
//! * **Is:** the scratch type, its `thread_local` owner, the
//!   enter/exit/borrow plumbing, and the free `commit_builtin`. All
//!   pure Rust, exercised by this module's own tests — no JS, no
//!   wasm-bindgen import.
//! * **Is not:** the `js_run_script_feature` Rust→JS import, the
//!   `ScriptFeature` `FeatureKind`, or the worker that hosts `build()`.
//!   Those are S2b/S2c and build directly on the surface here.

use std::cell::RefCell;
use std::collections::HashMap;
use std::str::FromStr;

use serde_json::Value;

use fielddraft_protocol::{Document, FeatureId};

use crate::csg::{DecodedFeature, EdgeFillet};
use crate::features::{Context, FeatureError, FeatureRegistry};
use crate::sketch_features::SketchData;

/// The geometry a script's `build()` accumulates across its `fd.op*`
/// calls — exactly the fields a regeneration [`Context`] owns, minus
/// the borrowed `Document` (which can't live in a `'static`
/// `thread_local`, and which `commit_builtin` doesn't need: built-in
/// ops read their inputs from the scratch, not the feature tree).
#[derive(Debug, Default)]
pub struct RegenScratch {
    /// The decoded feature spine, growing one entry per committed op.
    pub features: Vec<DecodedFeature>,
    /// Solved sketch state, keyed by sketch feature id.
    pub sketches: HashMap<FeatureId, SketchData>,
    /// Edge fillets collected for the post-processing pass.
    pub edge_fillets: Vec<EdgeFillet>,
}

impl RegenScratch {
    /// An empty scratch — the starting point of a fresh script run.
    pub fn new() -> Self {
        Self::default()
    }
}

thread_local! {
    /// The live scratch for the script callback currently on the stack.
    /// `Some` only between [`enter`] and [`exit`]; the re-entrant
    /// commits borrow it briefly and disjointly (single-threaded wasm,
    /// so the borrows never overlap).
    static SCRATCH: RefCell<Option<RegenScratch>> = const { RefCell::new(None) };
}

/// Errors raised by the script-callback boundary.
#[derive(Debug, Clone, PartialEq)]
pub enum ScriptError {
    /// A commit was attempted with no scratch installed — a re-entrant
    /// call leaked outside an `enter`/`exit` span (a programming error
    /// in the boundary, never reachable from a well-formed callback).
    NoScratch,
    /// The op-descriptor's `id` string didn't parse as a [`FeatureId`].
    BadId(String),
    /// The underlying built-in rejected the op (unknown type, invalid
    /// params, …). Carries the feature-layer error verbatim.
    Feature(FeatureError),
}

impl std::fmt::Display for ScriptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoScratch => f.write_str("no script scratch is installed"),
            Self::BadId(s) => write!(f, "invalid feature id `{s}`"),
            Self::Feature(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ScriptError {}

impl From<FeatureError> for ScriptError {
    fn from(e: FeatureError) -> Self {
        Self::Feature(e)
    }
}

/// Install `scratch` as the live accumulator for a script callback.
///
/// Called by `ScriptFeature::execute` (S2b) right before it invokes the
/// JS `build()`. Panics if a scratch is already installed: scripts
/// nest by **pure JS recursion** sharing the *same* scratch
/// (`fd.call`, §2.4), so the `thread_local` is only ever entered once
/// per top-level script run — a second `enter` means the previous run
/// wasn't drained and is a boundary bug.
pub fn enter(scratch: RegenScratch) {
    SCRATCH.with(|cell| {
        let mut slot = cell.borrow_mut();
        assert!(slot.is_none(), "script scratch entered re-entrantly");
        *slot = Some(scratch);
    });
}

/// Take the live scratch back out once `build()` has returned.
///
/// Panics if no scratch is installed — `exit` must pair with a prior
/// [`enter`].
pub fn exit() -> RegenScratch {
    SCRATCH.with(|cell| {
        cell.borrow_mut()
            .take()
            .expect("script scratch exited without a matching enter")
    })
}

/// Borrow the live scratch mutably for the duration of `f`.
///
/// The borrow is released the instant `f` returns, keeping every
/// re-entry's hold on the `RefCell` short and disjoint (§2.6). Errors
/// with [`ScriptError::NoScratch`] when called outside an
/// `enter`/`exit` span.
pub fn with_scratch<R>(f: impl FnOnce(&mut RegenScratch) -> R) -> Result<R, ScriptError> {
    SCRATCH.with(|cell| {
        let mut slot = cell.borrow_mut();
        let scratch = slot.as_mut().ok_or(ScriptError::NoScratch)?;
        Ok(f(scratch))
    })
}

/// Commit one built-in op into the live scratch and return the ids of
/// the feature(s) it created (the minimal query handle: a `createdBy`
/// reference for a later `fd.q` resolve, S2b+).
///
/// This is the mutating half of `fd.op*`. It reuses the built-in's own
/// `execute` so the geometry logic is the single Rust path shared with
/// the 95 % native loop — `eval` (pure) and `commit` (mutate) collapse
/// into one call because the IR never has to cross into JS.
///
/// `registry` is the built-in table; the eventual wasm free function
/// rebuilds [`FeatureRegistry::with_builtins`] per call (it is cheap
/// and stateless). `id` is the op's hierarchical id string
/// (`"3.bore"`), `type_name` the built-in to run, `params` its already
/// variable-resolved parameter blob.
pub fn commit_builtin(
    registry: &FeatureRegistry,
    id: &str,
    type_name: &str,
    params: &Value,
) -> Result<Vec<String>, ScriptError> {
    let feature_id = FeatureId::from_str(id).map_err(|_| ScriptError::BadId(id.to_string()))?;
    let feature = registry
        .get(type_name)
        .ok_or_else(|| ScriptError::Feature(FeatureError::UnknownFeatureType(type_name.to_string())))?;

    with_scratch(|scratch| {
        // Borrow the built-in's `execute` by lending it a throwaway
        // Context whose accumulator *is* the scratch. The Document is a
        // fresh empty one: built-in ops read their geometry inputs from
        // the context (sketch state, prior features), not from the
        // persisted feature tree, so an empty Document is sound here and
        // keeps the scratch free of any borrow that would outlive the
        // call.
        let mut doc = Document::default();
        let mut ctx = Context::new(&mut doc);
        std::mem::swap(&mut ctx.features, &mut scratch.features);
        std::mem::swap(&mut ctx.sketches, &mut scratch.sketches);
        std::mem::swap(&mut ctx.edge_fillets, &mut scratch.edge_fillets);

        let before = ctx.features.len();
        let result = feature.execute(&mut ctx, &feature_id, params);

        // Whether the op succeeded or failed, move the (possibly
        // partially-mutated) accumulator back so the scratch always
        // reflects the live state — fail-stop happens one level up.
        std::mem::swap(&mut ctx.features, &mut scratch.features);
        std::mem::swap(&mut ctx.sketches, &mut scratch.sketches);
        std::mem::swap(&mut ctx.edge_fillets, &mut scratch.edge_fillets);

        result?;
        let handles = scratch.features[before..]
            .iter()
            .map(|f| f.id.to_string())
            .collect();
        Ok(handles)
    })?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::FeatureCall;
    use serde_json::json;

    /// Run the registry's normal regenerate for a single feature call
    /// and return the decoded spine — the reference output a script
    /// commit must match.
    fn regenerate_one(type_name: &str, id: &str, params: Value) -> Vec<DecodedFeature> {
        let registry = FeatureRegistry::with_builtins();
        let mut doc = Document::default();
        let mut ctx = Context::new(&mut doc);
        let call = FeatureCall {
            type_name: type_name.to_string(),
            id: FeatureId::from_str(id).unwrap(),
            params,
            suppressed: false,
        };
        registry.regenerate(&mut ctx, &[call], None).unwrap();
        ctx.features
    }

    fn box_params() -> Value {
        json!({ "half_extents": [1.0, 2.0, 3.0], "center": [0.0, 0.0, 0.0] })
    }

    #[test]
    fn commit_matches_regenerate_for_a_leaf_primitive() {
        let registry = FeatureRegistry::with_builtins();
        enter(RegenScratch::new());
        let handles = commit_builtin(&registry, "7", "box", &box_params()).unwrap();
        let scratch = exit();

        assert_eq!(handles, vec!["7".to_string()]);
        let reference = regenerate_one("box", "7", box_params());
        assert_eq!(scratch.features, reference);
    }

    #[test]
    fn commits_accumulate_in_order() {
        let registry = FeatureRegistry::with_builtins();
        enter(RegenScratch::new());
        commit_builtin(&registry, "1", "box", &box_params()).unwrap();
        commit_builtin(
            &registry,
            "2",
            "sphere",
            &json!({ "radius": 1.5, "center": [0.0, 0.0, 0.0] }),
        )
        .unwrap();
        let scratch = exit();

        assert_eq!(scratch.features.len(), 2);
        assert_eq!(scratch.features[0].id.to_string(), "1");
        assert_eq!(scratch.features[1].id.to_string(), "2");
    }

    #[test]
    fn hierarchical_sub_ids_round_trip() {
        let registry = FeatureRegistry::with_builtins();
        enter(RegenScratch::new());
        let handles = commit_builtin(&registry, "3.bore", "box", &box_params()).unwrap();
        let scratch = exit();

        assert_eq!(handles, vec!["3.bore".to_string()]);
        assert_eq!(scratch.features[0].id.root_ancestor().to_string(), "3");
    }

    #[test]
    fn unknown_type_errors_and_leaves_scratch_untouched() {
        let registry = FeatureRegistry::with_builtins();
        enter(RegenScratch::new());
        commit_builtin(&registry, "1", "box", &box_params()).unwrap();
        let err = commit_builtin(&registry, "2", "not_a_feature", &json!({})).unwrap_err();
        let scratch = exit();

        assert!(matches!(err, ScriptError::Feature(FeatureError::UnknownFeatureType(_))));
        assert_eq!(scratch.features.len(), 1, "failed op must not add a feature");
    }

    #[test]
    fn bad_id_errors() {
        let registry = FeatureRegistry::with_builtins();
        enter(RegenScratch::new());
        let err = commit_builtin(&registry, "", "box", &box_params()).unwrap_err();
        let _ = exit();
        assert!(matches!(err, ScriptError::BadId(_)));
    }

    #[test]
    fn commit_outside_a_span_reports_no_scratch() {
        let registry = FeatureRegistry::with_builtins();
        let err = commit_builtin(&registry, "1", "box", &box_params()).unwrap_err();
        assert_eq!(err, ScriptError::NoScratch);
    }

    #[test]
    fn enter_exit_round_trips_a_scratch() {
        let mut scratch = RegenScratch::new();
        scratch.features.push(regenerate_one("box", "5", box_params()).remove(0));
        enter(scratch);
        with_scratch(|s| assert_eq!(s.features.len(), 1)).unwrap();
        let back = exit();
        assert_eq!(back.features[0].id.to_string(), "5");
    }
}
