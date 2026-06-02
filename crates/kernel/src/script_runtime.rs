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
    /// Lazily woven B-Reps of the in-flight geometry, one per body
    /// partition. Keyed on a content hash of `features`; a commit that
    /// changes `features` invalidates it (next query re-weaves). `None`
    /// until the first [`RegenScratch::ensure_breps`] call, and reset to
    /// `None` by every successful [`commit_builtin`].
    ///
    /// This is the read side of the live context (S3): a script's `fd.q`
    /// resolves element-level queries against these woven solids while
    /// `build()` is still running, mirroring [`Body::ensure_brep`]'s
    /// content-keyed laziness but at scratch (whole-document) granularity.
    brep_cache: Option<(u64, Vec<crate::brep::BRep>)>,
}

impl RegenScratch {
    /// An empty scratch — the starting point of a fresh script run.
    pub fn new() -> Self {
        Self::default()
    }

    /// Return the lazily woven B-Reps of the in-flight geometry, one per
    /// body partition, weaving them on first access (or after a commit
    /// invalidated the cache) and caching the result.
    ///
    /// The cache key is a content hash of the current `features` spine
    /// ([`body::hash_body_payload`] over every feature). A commit that
    /// appends or mutates a feature changes the key and forces a
    /// re-weave; a pure read re-uses the cache. `commit_builtin` also
    /// clears the cache explicitly, so the key-mismatch path is a
    /// belt-and-braces safety net rather than the primary invalidation.
    ///
    /// Each body is built via the canonical partition→`Body`→`ensure_brep`
    /// recipe (the same path `Kernel::rebuild_bodies` uses), so the woven
    /// element structure — and therefore the stable id hashes a query
    /// returns — matches what a persisted regeneration would produce.
    pub fn ensure_breps(&mut self) -> &[crate::brep::BRep] {
        use crate::body::{hash_body_payload, partition, Body};
        use fielddraft_protocol::Transform;

        let all: Vec<usize> = (0..self.features.len()).collect();
        let key = hash_body_payload(&self.features, &all);

        let fresh = !matches!(&self.brep_cache, Some((k, _)) if *k == key);
        if fresh {
            let parts = partition(&self.features);
            let mut breps = Vec::with_capacity(parts.len());
            for part in &parts {
                let mut body = Body {
                    id: part.seed_id(&self.features),
                    feature_indices: part.feature_indices.clone(),
                    aabb: part.aabb,
                    payload_hash: 0,
                    geometry_hash: 0,
                    transform_hash: 0,
                    mesh_resolution: 0,
                    mesh: crate::dc::Mesh::default(),
                    version: 1,
                    transform_delta: None,
                    last_mesh_transforms: Vec::new(),
                    compiled_sdf: None,
                    placement: Transform::IDENTITY,
                    brep: None,
                };
                body.ensure_brep(&self.features);
                breps.push(body.brep.expect("ensure_brep populates the cache").1);
            }
            self.brep_cache = Some((key, breps));
        }
        &self.brep_cache.as_ref().unwrap().1
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
    /// An op-descriptor's params blob wasn't valid JSON.
    BadParams(String),
    /// A script feature was reached but no JS runner is installed — a
    /// headless/native regenerate of a document that contains a script
    /// feature (§2.7: pure-Rust docs regenerate without a JS runtime,
    /// only script docs need one).
    NoRunner,
    /// The JS `build()` threw. Carries the JS-side message.
    JsRun(String),
    /// A `fd.q` query string didn't parse. Carries the parser's message
    /// (the read-side counterpart to [`ScriptError::BadParams`]).
    BadQuery(String),
    /// The underlying built-in rejected the op (unknown type, invalid
    /// params, …). Carries the feature-layer error verbatim.
    Feature(FeatureError),
}

impl std::fmt::Display for ScriptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoScratch => f.write_str("no script scratch is installed"),
            Self::BadId(s) => write!(f, "invalid feature id `{s}`"),
            Self::BadParams(s) => write!(f, "invalid op params json: {s}"),
            Self::NoRunner => f.write_str("no script runner is installed"),
            Self::JsRun(s) => write!(f, "script build() failed: {s}"),
            Self::BadQuery(s) => write!(f, "invalid query: {s}"),
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

        // A successful op mutated `features`, so any previously woven
        // B-Reps are stale: drop the cache so the next `fd.q` re-weaves
        // against the new geometry. (The content-key check in
        // `ensure_breps` would also catch this, but clearing here keeps
        // the invalidation obvious and local to the write path — §2.6.)
        scratch.brep_cache = None;

        result?;
        let handles = scratch.features[before..]
            .iter()
            .map(|f| f.id.to_string())
            .collect();
        Ok(handles)
    })?
}

/// The synchronous Rust→JS hop: runs one script's `build(context, id,
/// def)` to completion. On wasm this closure calls the JS function the
/// worker registered (which executes `build()` in the determinism
/// sandbox and re-enters via [`commit_builtin_global`]); in native
/// tests it's a Rust stub that drives commits directly.
///
/// The three arguments are `(script_id, instance_id, params_json)`:
/// `script_id` selects which `build()` to run (= the script-type id),
/// `instance_id` is the feature-call id this instance was placed under
/// (so `build()` can derive collision-free sub-op ids like `"10.a"`),
/// and `params_json` is the variable-resolved `def`.
///
/// Not `Send` — on wasm it captures a `js_sys::Function`, and the whole
/// runtime is single-threaded — so it lives in a `thread_local`, never
/// in the `Send + Sync` feature registry.
pub type ScriptRunFn = Box<dyn Fn(&str, &str, &str) -> Result<(), ScriptError>>;

thread_local! {
    /// The script runner for the current regenerate pass. Installed by
    /// the kernel around `regenerate` (wasm: wrapping the worker's JS
    /// callback), `None` for a pure-Rust headless run.
    static RUNNER: RefCell<Option<ScriptRunFn>> = const { RefCell::new(None) };
}

/// Install the script runner for the upcoming regenerate pass. Replaces
/// any previous runner.
pub fn set_runner(runner: ScriptRunFn) {
    RUNNER.with(|cell| *cell.borrow_mut() = Some(runner));
}

/// Remove the script runner once regeneration is done.
pub fn clear_runner() {
    RUNNER.with(|cell| *cell.borrow_mut() = None);
}

/// Drive one script's `build()` through the installed runner.
///
/// The runner is held borrowed for the duration of the JS call. That is
/// sound because a script's `build()` never re-enters *this* function:
/// script-to-script composition (`fd.call`) recurses in pure JS sharing
/// the live scratch (§2.4), and the re-entrant `fd.op*` commits go
/// through the *separate* [`SCRATCH`] thread-local — disjoint borrows.
pub fn run_script(
    script_id: &str,
    instance_id: &str,
    params_json: &str,
) -> Result<(), ScriptError> {
    RUNNER.with(|cell| {
        let borrow = cell.borrow();
        let runner = borrow.as_ref().ok_or(ScriptError::NoRunner)?;
        runner(script_id, instance_id, params_json)
    })
}

/// The global commit entry the JS `fd.op*` re-enters: rebuilds the
/// stateless built-in registry, parses the op params, and commits into
/// the live scratch. Returns the created-feature handles.
///
/// A free function with no kernel `self` — that is exactly what lets it
/// be called re-entrantly while `regenerate` holds `&mut Kernel` on the
/// stack (§2.6).
pub fn commit_builtin_global(
    id: &str,
    type_name: &str,
    params_json: &str,
) -> Result<Vec<String>, ScriptError> {
    let params: Value =
        serde_json::from_str(params_json).map_err(|e| ScriptError::BadParams(e.to_string()))?;
    let registry = FeatureRegistry::with_builtins();
    commit_builtin(&registry, id, type_name, &params)
}

/// JSON DTO for one resolved query handle (the frozen cross-language
/// contract for `fd.q`).
///
/// We deliberately do **not** serialise a [`TopoRef`](crate::brep::naming::TopoRef)
/// directly: its `id_hash` is a `u64`, which loses precision when a JS
/// runtime reads it as a `number`. So the hash is emitted as a decimal
/// **string** and the handle carries only the JS-safe fields a script
/// needs to round-trip a reference. `contributors` is the query's own
/// input feature ids ([`FeatureId`] `Display` strings) — what the script
/// asked for, which makes the handle self-describing; element-derived
/// contributors can come later.
#[derive(serde::Serialize)]
struct QueryHandleDto {
    /// Lowercase entity kind: `"face"`, `"edge"`, or `"vertex"`.
    kind: String,
    /// The `u64` id hash as a decimal string (JS-number-safe).
    #[serde(rename = "idHash")]
    id_hash: String,
    /// The query's input feature ids as `FeatureId` display strings.
    contributors: Vec<String>,
}

/// Parse + resolve a query against the live scratch geometry, returning
/// the resolved element handles as JSON.
///
/// Weaves the in-flight B-Rep lazily (cached on the scratch, invalidated
/// by every commit). This is the **read** side of the §2.6 single-writer
/// discipline: it takes the same short, disjoint [`with_scratch`] borrow
/// the commit path uses, never holding a borrow across re-entry. Called
/// outside an `enter`/`exit` span it surfaces [`ScriptError::NoScratch`]
/// (a query asked outside a `build()`); a malformed query string is
/// [`ScriptError::BadQuery`].
///
/// The returned string is a JSON array of [`QueryHandleDto`]; an empty,
/// well-formed match is `"[]"`.
pub fn resolve_query_global(query: &str) -> Result<String, ScriptError> {
    use crate::brep::naming::EntityKind;

    let expr = crate::queries::parse_query(query)
        .map_err(|e| ScriptError::BadQuery(e.to_string()))?;

    // Contributors are the query's own input feature ids (Display form).
    let contributors: Vec<String> = match &expr {
        crate::queries::QueryExpr::CreatedBy(f)
        | crate::queries::QueryExpr::FaceOf(f)
        | crate::queries::QueryExpr::AllRegions(f)
        | crate::queries::QueryExpr::Region(f, _)
        | crate::queries::QueryExpr::Regions(f, _) => vec![f.to_string()],
        crate::queries::QueryExpr::EdgeBetween(a, b) => vec![a.to_string(), b.to_string()],
    };

    let refs = with_scratch(|s| {
        let breps = s.ensure_breps();
        crate::queries::resolve_query_topo(breps, &expr)
    })?;

    let dtos: Vec<QueryHandleDto> = refs
        .iter()
        .map(|r| QueryHandleDto {
            kind: match r.kind {
                EntityKind::Vertex => "vertex",
                EntityKind::Edge => "edge",
                EntityKind::Face => "face",
            }
            .to_string(),
            id_hash: r.id_hash.to_string(),
            contributors: contributors.clone(),
        })
        .collect();

    serde_json::to_string(&dtos).map_err(|e| ScriptError::BadQuery(e.to_string()))
}

/// A FieldSkript feature: a runtime-registered [`FeatureKind`] whose
/// geometry comes from a JS `build()` rather than native Rust.
///
/// Holds only its stable id (= `FeatureCall.type_name` = `record.id`)
/// and its schema, so it stays `Send + Sync` and the registry treats it
/// exactly like a built-in. `execute` hands the live accumulator to the
/// script runner and folds the script's committed geometry back into
/// the regeneration context.
pub struct ScriptFeature {
    id: String,
    schema: Value,
}

impl ScriptFeature {
    /// Build a script feature kind from a persisted record's id +
    /// schema (see `FieldSkriptRecord`).
    pub fn new(id: impl Into<String>, schema: Value) -> Self {
        Self {
            id: id.into(),
            schema,
        }
    }
}

impl crate::features::FeatureKind for ScriptFeature {
    fn type_name(&self) -> &str {
        &self.id
    }

    fn schema_json(&self) -> Value {
        self.schema.clone()
    }

    fn execute(
        &self,
        ctx: &mut Context<'_>,
        id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        // Hand the in-flight accumulator to the script as its live
        // context, run build(), then fold whatever it committed back in.
        // A fresh scratch is *not* used: the script must see the
        // geometry earlier features produced (Onshape's "created by an
        // earlier operation").
        let scratch = RegenScratch {
            features: std::mem::take(&mut ctx.features),
            sketches: std::mem::take(&mut ctx.sketches),
            edge_fillets: std::mem::take(&mut ctx.edge_fillets),
            brep_cache: None,
        };
        enter(scratch);

        let params_json = serde_json::to_string(def).unwrap_or_else(|_| "{}".to_string());
        let result = run_script(&self.id, &id.to_string(), &params_json);

        let scratch = exit();
        ctx.features = scratch.features;
        ctx.sketches = scratch.sketches;
        ctx.edge_fillets = scratch.edge_fillets;

        result.map_err(|e| FeatureError::InvalidParameter {
            feature: "fieldskript",
            name: "build",
            message: e.to_string(),
        })
    }
}

/// wasm-bindgen surface for the JS→Rust commit re-entry. The mutating
/// half of `fd.op*`: the worker's `fd.opBox(...)` etc. call `fdOpCommit`
/// synchronously while the kernel's `regenerate` is on the stack. It is
/// a *free* export, never a `Kernel` method, so the re-entrant call
/// doesn't recursively borrow the already-borrowed kernel object
/// (§2.6). The Rust→JS half (the script runner) is wired in S2c with
/// the worker.
#[cfg(target_arch = "wasm32")]
mod wasm_exports {
    use wasm_bindgen::prelude::*;

    /// Commit one built-in op into the live script scratch; returns the
    /// created-feature handle ids as a JSON array.
    #[wasm_bindgen(js_name = fdOpCommit)]
    pub fn fd_op_commit(id: &str, type_name: &str, params_json: &str) -> Result<String, JsError> {
        let handles = super::commit_builtin_global(id, type_name, params_json)
            .map_err(|e| JsError::new(&e.to_string()))?;
        serde_json::to_string(&handles).map_err(|e| JsError::new(&e.to_string()))
    }

    /// Resolve a `fd.q` query against the live script scratch; returns a
    /// JSON array of element handles (`{ kind, idHash, contributors }`).
    /// The read-side re-entry of the live context (S3): the worker calls
    /// this synchronously from `build()` while `regenerate` is on the
    /// stack. A malformed query or a query outside a `build()` span
    /// becomes a thrown JS error.
    #[wasm_bindgen(js_name = fdResolveQuery)]
    pub fn fd_resolve_query(query: &str) -> Result<String, JsError> {
        super::resolve_query_global(query).map_err(|e| JsError::new(&e.to_string()))
    }

    /// Install the worker's JS `build()` driver as the script runner for
    /// this realm. The Rust→JS half of `fd.op*`: when `regenerate`
    /// reaches a [`super::ScriptFeature`] it calls `run(scriptId,
    /// instanceId, paramsJson)`, which the worker implements by running
    /// the matching compiled `build()` in the determinism sandbox. A
    /// thrown JS error becomes [`super::ScriptError::JsRun`] and
    /// fail-stops the regeneration. Installed once at worker boot — the
    /// runner persists in the `thread_local` across rebuilds.
    #[wasm_bindgen(js_name = installScriptRunner)]
    pub fn install_script_runner(run: js_sys::Function) {
        super::set_runner(Box::new(
            move |script_id: &str, instance_id: &str, params_json: &str| {
                run.call3(
                    &JsValue::NULL,
                    &JsValue::from_str(script_id),
                    &JsValue::from_str(instance_id),
                    &JsValue::from_str(params_json),
                )
                .map(|_| ())
                .map_err(|e| super::ScriptError::JsRun(js_error_message(&e)))
            },
        ));
    }

    /// Best-effort extraction of a readable message from a thrown JS
    /// value (an `Error` instance gives its `.message`; anything else
    /// falls back to its debug form).
    fn js_error_message(value: &JsValue) -> String {
        value
            .dyn_ref::<js_sys::Error>()
            .map(|err| String::from(err.message()))
            .unwrap_or_else(|| format!("{value:?}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::{FeatureCall, FeatureKind};
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

    // ── S2b: ScriptFeature dispatch via the regenerate loop ─────────

    /// Build a registry with the built-ins plus one script feature, and
    /// regenerate `calls` against it with `runner` installed for the
    /// pass — mirroring how the kernel will drive a script document.
    fn regenerate_with_script(
        script_id: &str,
        runner: ScriptRunFn,
        calls: &[FeatureCall],
    ) -> Result<Vec<DecodedFeature>, FeatureError> {
        let mut registry = FeatureRegistry::with_builtins();
        registry.register(Box::new(ScriptFeature::new(script_id, json!({}))));
        let mut doc = Document::default();
        let mut ctx = Context::new(&mut doc);
        set_runner(runner);
        let result = registry.regenerate(&mut ctx, calls, None);
        clear_runner();
        result.map(|()| ctx.features)
    }

    fn script_call(type_name: &str, id: &str) -> FeatureCall {
        FeatureCall {
            type_name: type_name.to_string(),
            id: FeatureId::from_str(id).unwrap(),
            params: json!({}),
            suppressed: false,
        }
    }

    #[test]
    fn script_feature_commits_its_build_geometry() {
        // The runner simulates a build() that emits two primitives.
        let runner: ScriptRunFn = Box::new(|_script_id, _instance_id, _params| {
            commit_builtin_global("10.a", "box", &box_params().to_string())?;
            commit_builtin_global(
                "10.b",
                "sphere",
                &json!({ "radius": 1.0, "center": [0.0, 0.0, 0.0] }).to_string(),
            )?;
            Ok(())
        });
        let spine = regenerate_with_script("counterbore", runner, &[script_call("counterbore", "10")])
            .unwrap();

        assert_eq!(spine.len(), 2, "both committed primitives land on the spine");
        assert_eq!(spine[0].id.to_string(), "10.a");
        assert_eq!(spine[1].id.to_string(), "10.b");
    }

    #[test]
    fn script_sees_geometry_from_earlier_features() {
        // A native box precedes the script; the script's build() must
        // observe it in the live scratch (created-by-earlier-operation).
        let runner: ScriptRunFn = Box::new(|_script_id, _instance_id, _params| {
            let seen = with_scratch(|s| s.features.len())?;
            assert_eq!(seen, 1, "the prior box is visible to build()");
            commit_builtin_global("9.boss", "box", &box_params().to_string())?;
            Ok(())
        });
        let calls = [
            FeatureCall {
                type_name: "box".to_string(),
                id: FeatureId::from_str("1").unwrap(),
                params: box_params(),
                suppressed: false,
            },
            script_call("counterbore", "9"),
        ];
        let spine = regenerate_with_script("counterbore", runner, &calls).unwrap();
        assert_eq!(spine.len(), 2);
        assert_eq!(spine[1].id.to_string(), "9.boss");
    }

    #[test]
    fn script_receives_its_instance_id() {
        // build() needs the feature-call id it was placed under to derive
        // collision-free sub-op ids; the runner must be handed it.
        use std::cell::RefCell;
        use std::rc::Rc;
        let seen_id = Rc::new(RefCell::new(String::new()));
        let captured = Rc::clone(&seen_id);
        let runner: ScriptRunFn = Box::new(move |_script_id, instance_id, _params| {
            *captured.borrow_mut() = instance_id.to_string();
            commit_builtin_global(&format!("{instance_id}.a"), "box", &box_params().to_string())?;
            Ok(())
        });
        let spine =
            regenerate_with_script("counterbore", runner, &[script_call("counterbore", "42")])
                .unwrap();
        assert_eq!(*seen_id.borrow(), "42");
        assert_eq!(spine[0].id.to_string(), "42.a");
    }

    #[test]
    fn script_without_a_runner_fails_the_feature() {
        let mut registry = FeatureRegistry::with_builtins();
        registry.register(Box::new(ScriptFeature::new("counterbore", json!({}))));
        let mut doc = Document::default();
        let mut ctx = Context::new(&mut doc);
        clear_runner();
        let err = registry
            .regenerate(&mut ctx, &[script_call("counterbore", "10")], None)
            .unwrap_err();
        assert!(matches!(err, FeatureError::InvalidParameter { feature: "fieldskript", .. }));
    }

    #[test]
    fn script_feature_exposes_its_schema() {
        let schema = json!({ "type": "counterbore", "params": [] });
        let feature = ScriptFeature::new("counterbore", schema.clone());
        assert_eq!(feature.type_name(), "counterbore");
        assert_eq!(feature.schema_json(), schema);
    }

    // ── S3: live read-context + element-query resolution ────────────

    fn sphere_params() -> Value {
        json!({ "radius": 1.5, "center": [0.0, 0.0, 0.0] })
    }

    /// Parse the resolve JSON and assert the frozen contract shape: an
    /// array of `{ kind, idHash, contributors }` objects whose `idHash`
    /// is a decimal string. Returns the parsed array.
    fn assert_handle_shape(json: &str) -> Vec<Value> {
        let v: Value = serde_json::from_str(json).expect("resolve output is valid JSON");
        let arr = v.as_array().expect("resolve output is a JSON array").clone();
        for h in &arr {
            let obj = h.as_object().expect("each handle is an object");
            assert!(obj.contains_key("kind"), "handle has `kind`");
            assert!(obj.contains_key("idHash"), "handle has `idHash`");
            assert!(obj.contains_key("contributors"), "handle has `contributors`");
            // idHash must be a string (u64 safety), parseable as a number.
            let id_hash = obj["idHash"].as_str().expect("idHash is a string");
            id_hash.parse::<u64>().expect("idHash is a decimal u64 string");
            assert!(obj["contributors"].is_array(), "contributors is an array");
        }
        arr
    }

    #[test]
    fn resolve_query_global_returns_box_faces() {
        let registry = FeatureRegistry::with_builtins();
        enter(RegenScratch::new());
        commit_builtin(&registry, "5", "box", &box_params()).unwrap();

        let json = resolve_query_global("qCreatedBy(5)").unwrap();
        let handles = assert_handle_shape(&json);
        assert_eq!(handles.len(), 6, "a box weaves to 6 faces");
        for h in &handles {
            assert_eq!(h["kind"], "face", "all created-by handles are faces");
            assert_eq!(
                h["contributors"].as_array().unwrap(),
                &vec![Value::from("5")],
                "contributor is the queried feature id"
            );
        }
        let _ = exit();
    }

    #[test]
    fn resolve_query_global_reweaves_after_a_commit() {
        let registry = FeatureRegistry::with_builtins();
        enter(RegenScratch::new());
        commit_builtin(&registry, "5", "box", &box_params()).unwrap();

        let before = resolve_query_global("qCreatedBy(5)").unwrap();
        let box_faces = assert_handle_shape(&before);
        assert_eq!(box_faces.len(), 6);

        // A second commit invalidates the brep_cache; a re-resolve of the
        // SAME query must still return the box's 6 faces (unchanged by the
        // new disjoint body) but produced by a FRESH weave — asserted by
        // content equality, not pointer identity.
        commit_builtin(&registry, "6", "sphere", &sphere_params()).unwrap();
        let after = resolve_query_global("qCreatedBy(5)").unwrap();
        let box_faces_again = assert_handle_shape(&after);
        assert_eq!(box_faces_again.len(), 6, "box faces unchanged after re-weave");
        assert_eq!(before, after, "same query, same box geometry → identical handles");

        // And the new feature is now resolvable too — proving the cache
        // did invalidate and re-weave to include it.
        let sphere_q = resolve_query_global("qCreatedBy(6)").unwrap();
        let sphere_faces = assert_handle_shape(&sphere_q);
        assert!(!sphere_faces.is_empty(), "the freshly committed sphere resolves");
        let _ = exit();
    }

    #[test]
    fn resolve_query_global_empty_for_unknown_feature() {
        let registry = FeatureRegistry::with_builtins();
        enter(RegenScratch::new());
        commit_builtin(&registry, "5", "box", &box_params()).unwrap();
        let json = resolve_query_global("qCreatedBy(999)").unwrap();
        assert_eq!(json, "[]", "well-formed query, no match → empty array");
        let _ = exit();
    }

    #[test]
    fn resolve_query_global_outside_a_span_is_no_scratch() {
        let err = resolve_query_global("qCreatedBy(5)").unwrap_err();
        assert_eq!(err, ScriptError::NoScratch);
    }

    #[test]
    fn resolve_query_global_malformed_query_is_bad_query() {
        enter(RegenScratch::new());
        let err = resolve_query_global("qBogus(1)").unwrap_err();
        let _ = exit();
        assert!(matches!(err, ScriptError::BadQuery(_)));
    }

    #[test]
    fn ensure_breps_caches_until_a_commit() {
        let registry = FeatureRegistry::with_builtins();
        enter(RegenScratch::new());
        commit_builtin(&registry, "5", "box", &box_params()).unwrap();

        // Two reads with no intervening commit hit the same cached weave
        // (same content, and the second call does not re-partition).
        let a = with_scratch(|s| s.ensure_breps().len()).unwrap();
        let b = with_scratch(|s| s.ensure_breps().len()).unwrap();
        assert_eq!(a, 1, "one body partition for a single box");
        assert_eq!(b, 1);

        // After a commit of a second disjoint body, the cache grows.
        commit_builtin(&registry, "6", "sphere", &sphere_params()).unwrap();
        let c = with_scratch(|s| s.ensure_breps().len()).unwrap();
        assert_eq!(c, 2, "two disjoint bodies after the sphere commit");
        let _ = exit();
    }
}
